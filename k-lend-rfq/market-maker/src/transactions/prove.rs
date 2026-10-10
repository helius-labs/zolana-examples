//! Proving: witnesses from the indexer, the proof from the prover, and a
//! local verification of it before any transaction is built. At most
//! `workers` proofs run at once; proving and the rpc work tied to it run off
//! the coordinator loop.

use std::sync::Arc;

use solana_address::Address;
use solana_hash::Hash;
use solana_instruction::Instruction;
use tokio::sync::Semaphore;
use zolana_client::{
    assemble, verify_confidential_transfer_inputs, AsyncProverClient, AsyncRpc, AsyncWitnessReader,
    AsyncZolanaIndexer, ClientError, InputWitnesses, NonInclusionProof, ProofAuthority,
    ProofCompressed,
};
use zolana_interface::pda;
use zolana_program::instruction::Transact;
use zolana_transaction::utxo::SppProofInputUtxo;

use k_lend_rfq_sdk::address::{
    add_order_address, OrderAddress, WitnessRequest, ORDER_ADDRESS_TREE,
};

use super::{
    confirm::Retry,
    coordinator::{Coordinator, Event},
    send::SendQueue,
    steps::{ProofWork, StepId, StepKind, StepState, TailShield},
    Identity,
};
use crate::error::MarketMakerError;

/// Proof attempts per step before its operation is requeued with fresh
/// inputs. A proof fails transiently when the prover or indexer is briefly
/// unavailable or behind; a third failure is treated as persistent. A
/// re-prove after a stale-root rejection (`handle_failure`, any non-fill
/// step including a rebalance with a kVault tail) counts against the same
/// limit, so a stale root costs a prove attempt, not one of the operation's
/// `OPERATION_ATTEMPTS`.
pub const PROVE_ATTEMPTS: u32 = 3;

/// A proven step, carried by `Event::Proven`.
pub struct ProvenStep {
    pub instruction: Instruction,
    /// Set exactly for fill steps: the latest blockhash and its last valid
    /// block height (or the error of fetching them), fetched in the prove
    /// task right after the proof so `offer_fill` compiles the swap message
    /// without an rpc call on the coordinator loop. Other steps fetch their
    /// blockhash in the send task.
    pub blockhash: Option<Result<(Hash, u64), MarketMakerError>>,
    /// Set exactly for steps with a `TailShield`: the kVault instruction and
    /// the shield resolved from a simulation of the proven transact, which
    /// replace the step's `tail`.
    pub tail: Option<Vec<Instruction>>,
}

/// What `ProofQueue::new` needs.
pub struct ProofQueueConfig {
    /// Completes the witness with the nullifier secret.
    pub authority: Arc<dyn ProofAuthority>,
    pub indexer: Arc<AsyncZolanaIndexer>,
    pub prover: Arc<AsyncProverClient>,
    /// Concurrent proofs; at least one is allowed.
    pub workers: usize,
    /// Fee payer named in every proven `transact`.
    pub payer: Address,
}

/// Bounded-concurrency prover front end.
pub struct ProofQueue {
    authority: Arc<dyn ProofAuthority>,
    indexer: Arc<AsyncZolanaIndexer>,
    prover: Arc<AsyncProverClient>,
    workers: Semaphore,
    payer: Address,
}

impl ProofQueue {
    /// A queue with `config.workers` workers (`Settings::new` rejects zero
    /// provers, so at least one).
    pub fn new(config: ProofQueueConfig) -> Self {
        Self {
            authority: config.authority,
            indexer: config.indexer,
            prover: config.prover,
            workers: Semaphore::new(config.workers),
            payer: config.payer,
        }
    }

    /// Waits for a worker, then proves `work` into a `transact` instruction.
    /// Errors with `MarketMakerError::CoordinatorStopped` once the queue is
    /// closed, with `MarketMakerError::Indexer` or `MarketMakerError::Prover`
    /// on a service failure, and with the client error when the proof does not
    /// verify locally.
    pub async fn prove(&self, work: ProofWork) -> Result<Instruction, MarketMakerError> {
        // Held until the proof finishes.
        let _worker_permit = self
            .workers
            .acquire()
            .await
            .map_err(|_| MarketMakerError::CoordinatorStopped)?;
        self.prove_transfer(work).await
    }

    /// Closes the worker semaphore: proofs that have not acquired a worker
    /// yet fail with `CoordinatorStopped`, running proofs finish.
    pub fn close(&self) {
        self.workers.close();
    }

    /// Inclusion proofs of the real inputs and non-inclusion proofs of the
    /// dummy nullifiers (followed by the order address when it shares their
    /// tree, see `WitnessRequest`). With no real input there is nothing to
    /// read inclusion for, so only the non-inclusion proofs are fetched,
    /// against the tree of `tree_id`.
    async fn witnesses(
        &self,
        tree_id: u16,
        commitments: &[&SppProofInputUtxo],
        dummy_nullifiers: &[[u8; 32]],
    ) -> Result<InputWitnesses, MarketMakerError> {
        if commitments.is_empty() {
            let dummy_nullifier_proofs = self
                .indexer
                .get_non_inclusion_proofs(pda::tree(tree_id), dummy_nullifiers.to_vec(), None)
                .await
                .map_err(MarketMakerError::Indexer)?
                .proofs;
            return Ok(InputWitnesses {
                spend_proofs: Vec::new(),
                dummy_nullifier_proofs,
            });
        }
        AsyncWitnessReader::input_witnesses(
            self.indexer.as_ref(),
            commitments,
            dummy_nullifiers,
            None,
        )
        .await
        .map_err(MarketMakerError::Indexer)
    }

    /// The non-inclusion proof of the order address of `request`, from the
    /// fetched padding proofs or, when the address tree is not the
    /// transfer's padding tree, from its own request to the order address
    /// tree.
    async fn address_proof(
        &self,
        request: &WitnessRequest,
        dummy_proofs: &mut Vec<NonInclusionProof>,
    ) -> Result<NonInclusionProof, MarketMakerError> {
        let separate = match request.separate_address() {
            Some(address) => self
                .indexer
                .get_non_inclusion_proofs(pda::tree(ORDER_ADDRESS_TREE), vec![address], None)
                .await
                .map_err(MarketMakerError::Indexer)?
                .proofs
                .into_iter()
                .next(),
            None => None,
        };
        request
            .address_proof(dummy_proofs, separate)
            .map_err(|error| MarketMakerError::AddressSlot(error.to_string()))
    }

    /// Assembles, proves and locally verifies one transfer. A fill's transfer
    /// carries its order address slot, owned by the fee payer and added by
    /// `add_order_address`. The local verification catches a bad proof before
    /// it costs a transaction fee.
    async fn prove_transfer(&self, work: ProofWork) -> Result<Instruction, MarketMakerError> {
        let ProofWork {
            inputs: proof_inputs,
            interface_accounts,
            address,
        } = work;
        let order = address
            .map(|id| OrderAddress::new(&self.payer, id))
            .transpose()
            .map_err(|error| MarketMakerError::AddressSlot(error.to_string()))?;
        let tree_id = proof_inputs
            .input_utxos
            .first()
            .map(|input| input.tree_id)
            .ok_or(ClientError::NoInputs)?;
        let request = WitnessRequest::new(&proof_inputs, tree_id, order.as_ref());
        let mut witnesses = self
            .witnesses(
                tree_id,
                &proof_inputs.input_utxo_hashes()?,
                &request.dummy_nullifiers,
            )
            .await?;
        let owner_signers = proof_inputs.owner_signer_pubkeys()?;
        let output_tree_pda = pda::tree(proof_inputs.output_tree_id);
        let (mut assembled, patch) = match &order {
            None => (
                assemble(
                    proof_inputs,
                    &witnesses.spend_proofs,
                    &witnesses.dummy_nullifier_proofs,
                )?,
                None,
            ),
            Some(order) => {
                let non_inclusion = self
                    .address_proof(&request, &mut witnesses.dummy_nullifier_proofs)
                    .await?;
                let (assembled, patch) = add_order_address(
                    proof_inputs,
                    &witnesses.spend_proofs,
                    &witnesses.dummy_nullifier_proofs,
                    order,
                    &non_inclusion,
                )
                .map_err(|error| MarketMakerError::AddressSlot(error.to_string()))?;
                (assembled, Some(patch))
            }
        };
        self.authority
            .complete_inputs(&mut assembled.prover_inputs.inputs)?;
        let inputs = &assembled.prover_inputs;
        let proof = self
            .prover
            .prove_transfer(inputs)
            .await
            .map_err(MarketMakerError::Prover)?;
        verify_confidential_transfer_inputs(inputs, assembled.public_input_hash, &proof)?;
        let input_trees = assembled
            .input_tree_ids
            .iter()
            .copied()
            .map(pda::tree)
            .collect();
        let mut data = assembled.with_proof(ProofCompressed::try_from(proof)?.to_transact_proof());
        if let Some(patch) = patch {
            patch.apply(&mut data);
        }
        Ok(Transact {
            payer: self.payer,
            input_trees,
            output_tree: output_tree_pda,
            owner_signers,
            interface_transfer_accounts: interface_accounts,
            data,
        }
        .instruction())
    }
}

impl Coordinator {
    /// Starts (or restarts) proving step `id`: resets it to
    /// `StepState::Proving`, counts the attempt, and reports through
    /// `Event::Proven` unless the coordinator is cancelled first.
    pub fn spawn_prove(&mut self, id: StepId) {
        let Some(step) = self.steps.get_mut(id) else {
            return;
        };
        let work = step.proof.clone();
        step.state = StepState::Proving;
        step.instruction = None;
        step.prove_attempts += 1;
        let fill = step.kind == StepKind::Fill;
        let tail_shield = step.tail_shield.clone();
        let identity = self.identity.clone();
        let proofs = self.services.proofs.clone();
        let sender = self.services.sender.clone();
        let events = self.runtime.events.clone();
        let cancel = self.runtime.cancel.clone();
        self.runtime.tasks.spawn(async move {
            let proven = prove_step(
                &proofs,
                &sender,
                work,
                fill,
                tail_shield.as_ref().map(|tail| (tail, &identity)),
            );
            if let Some(outcome) = cancel.run_until_cancelled(proven).await {
                let _ = events.send(Event::Proven { step: id, outcome });
            }
        });
    }

    /// Handles a finished proof. A fill step, whose proof carries a
    /// blockhash, goes to `offer_fill`; any other step is size-checked and
    /// sent. A failed proof is retried up to `PROVE_ATTEMPTS`, then the
    /// operation is requeued.
    pub async fn on_proven(&mut self, id: StepId, outcome: Result<ProvenStep, MarketMakerError>) {
        let Some(step) = self.steps.get_mut(id) else {
            return;
        };
        let proven = match outcome {
            Ok(proven) => proven,
            Err(error) if step.prove_attempts < PROVE_ATTEMPTS => {
                tracing::warn!(step = id, %error, "proof failed, retrying");
                self.spawn_prove(id);
                return;
            }
            Err(error) => {
                self.abort(id, error, Retry::Requeue).await;
                return;
            }
        };
        step.instruction = Some(proven.instruction);
        if let Some(tail) = proven.tail {
            step.tail = tail;
        }
        step.state = StepState::Proven;
        if let Some(blockhash) = proven.blockhash {
            self.offer_fill(id, blockhash).await;
            return;
        }
        let too_large = self
            .send_request(id)
            .and_then(|request| self.services.sender.check_size(&request).err());
        match too_large {
            Some(error) => self.abort(id, error, Retry::Fail).await,
            None => self.send_ready(),
        }
    }
}

/// Proves `work` and, for a fill, fetches the blockhash its swap message is
/// compiled with. For a step with a `TailShield`, simulates the proven
/// transact followed by the kVault instruction and resolves the shield
/// amount from the simulated balance (`TailShield::resolve`); a failed
/// simulation fails the step like a failed proof. Runs in the prove task,
/// off the coordinator loop.
async fn prove_step(
    proofs: &ProofQueue,
    sender: &SendQueue,
    work: ProofWork,
    fill: bool,
    tail_shield: Option<(&TailShield, &Identity)>,
) -> Result<ProvenStep, MarketMakerError> {
    let instruction = proofs.prove(work).await?;
    let blockhash = if fill {
        Some(sender.latest_blockhash().await)
    } else {
        None
    };
    let tail = match tail_shield {
        Some((tail, identity)) => Some(tail.resolve(sender, identity, Some(&instruction)).await?),
        None => None,
    };
    Ok(ProvenStep {
        instruction,
        blockhash,
        tail,
    })
}
