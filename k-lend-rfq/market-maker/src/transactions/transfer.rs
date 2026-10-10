//! The maker's shielded transfers: planning (inputs, payment, public
//! withdrawal, change parts), building and encrypting the transaction, and
//! admitting it as a step. A plan only exists when its parts add up to its
//! inputs exactly, so no value is created or lost by the maker's own outputs.

use std::{collections::BTreeSet, sync::Arc};

use solana_address::Address;
use solana_signature::Signature;
use zolana_client::Shape;
use zolana_interface::pda;
use zolana_keypair::ShieldedAddress;
use zolana_program::instruction::{
    TransactInterfaceTransferAccounts, TransactSplWithdrawalAccounts,
};
use zolana_transaction::{
    instructions::transact::ConfidentialTransaction, keys::DeriveRequest, Mint, ShieldedKeys,
    SppProofOutputUtxo, TransactionError, Utxo, WalletUtxo,
};

use k_lend_rfq_sdk::{pair::VaultState, transfer::smallest_shape};

use super::{
    coordinator::Coordinator,
    steps::{FillTransfer, OperationId, ProofWork, Step, StepId, StepKind, TailShield},
};
use crate::{
    error::MakerError, inventory::balance::select::Selection, transactions::budget::BudgetError,
};

/// Where a transfer's public withdrawal goes: `owner`'s associated token
/// account under `token_program`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WithdrawalTarget {
    pub owner: Address,
    pub token_program: Address,
}

impl WithdrawalTarget {
    /// `owner`'s associated token account of `mint`.
    pub fn token_account(&self, mint: &Address) -> Address {
        pda::associated_token_address_with_program(&self.owner, mint, &self.token_program)
    }

    /// The accounts of an SPL withdrawal of `mint` to `token_account`.
    pub fn spl_accounts(&self, mint: Address) -> TransactSplWithdrawalAccounts {
        TransactSplWithdrawalAccounts {
            mint,
            spl_interface: pda::spl_interface(&mint),
            user_token_account: self.token_account(&mint),
            token_program: self.token_program,
        }
    }
}

/// A checked transfer plan: `selection` pays the recipient, the public
/// withdrawal and `own_parts` back to the maker, which sum to exactly the
/// selected total.
#[derive(Clone)]
pub struct TransferPlan {
    pub selection: Selection,
    /// The payment to another party (a fill's user), if any.
    pub recipient: Option<(ShieldedAddress, u64)>,
    /// Amount withdrawn to the maker's public account (0 for none).
    pub withdrawal: u64,
    /// The maker's own outputs.
    pub own_parts: Vec<u64>,
    /// The narrowest supported shape for the inputs and outputs.
    pub shape: Shape,
}

impl TransferPlan {
    /// Errors with `MakerError::AmountOverflow` on overflowing sums,
    /// `MakerError::InsufficientBalance` when the selection does not cover
    /// payment plus withdrawal, `MakerError::OwnPartsMismatch` when
    /// `own_parts` do not sum to the rest, and
    /// `MakerError::Budget(BudgetError::NoSupportedShape)` when no shape fits.
    fn new(
        selection: Selection,
        recipient: Option<(ShieldedAddress, u64)>,
        withdrawal: u64,
        own_parts: Vec<u64>,
    ) -> Result<Self, MakerError> {
        let spent = recipient
            .map(|(_, amount)| amount)
            .unwrap_or(0)
            .checked_add(withdrawal)
            .ok_or(MakerError::AmountOverflow {
                context: "transfer spend",
            })?;
        let own_value = own_value(&selection, spent)?;
        let planned = own_parts
            .iter()
            .try_fold(0u64, |total, part| total.checked_add(*part))
            .ok_or(MakerError::AmountOverflow {
                context: "transfer own parts",
            })?;
        if planned != own_value {
            return Err(MakerError::OwnPartsMismatch { planned, own_value });
        }
        let outputs = usize::from(recipient.is_some()) + own_parts.len();
        let inputs = selection.inputs.len();
        let shape = smallest_shape(inputs, outputs)
            .ok_or(BudgetError::NoSupportedShape { inputs, outputs })?;
        Ok(Self {
            selection,
            recipient,
            withdrawal,
            own_parts,
            shape,
        })
    }
}

/// What `selection` keeps for the maker after spending `spent`; errors with
/// `MakerError::InsufficientBalance` when it does not cover `spent`.
pub fn own_value(selection: &Selection, spent: u64) -> Result<u64, MakerError> {
    selection
        .total
        .checked_sub(spent)
        .ok_or(MakerError::InsufficientBalance {
            asset: selection
                .inputs
                .first()
                .map(|input| input.utxo.asset())
                .unwrap_or_default(),
            available: selection.total,
            requested: spent,
        })
}

/// A plan paying `amount` to `recipient`, the rest as `own_parts`.
pub fn plan_transfer(
    selection: Selection,
    recipient: ShieldedAddress,
    amount: u64,
    own_parts: Vec<u64>,
) -> Result<TransferPlan, MakerError> {
    TransferPlan::new(selection, Some((recipient, amount)), 0, own_parts)
}

/// A plan withdrawing `withdrawal` publicly (0 for none), the rest as
/// `own_parts`.
pub fn plan_consolidate(
    selection: Selection,
    withdrawal: u64,
    own_parts: Vec<u64>,
) -> Result<TransferPlan, MakerError> {
    TransferPlan::new(selection, None, withdrawal, own_parts)
}

/// An encrypted transfer ready to prove.
#[derive(Clone)]
pub struct BuiltTransfer {
    pub proof: ProofWork,
    /// The maker's own non-zero outputs, with nullifiers, checked against
    /// their commitments.
    pub expected_outputs: Vec<WalletUtxo>,
}

/// Inputs to building a transfer from a plan.
#[derive(Clone)]
pub struct TransferBuild {
    pub plan: TransferPlan,
    pub own: ShieldedAddress,
    pub payer: Address,
    pub tree_id: u16,
    pub withdrawal: Option<WithdrawalTarget>,
}

impl TransferBuild {
    /// Builds the transfer on a blocking thread (encryption and key
    /// derivation are CPU-bound); errors with `MakerError::BlockingTask` if
    /// that thread fails.
    pub async fn run(
        self,
        keys: Arc<dyn ShieldedKeys + Send + Sync>,
    ) -> Result<BuiltTransfer, MakerError> {
        tokio::task::spawn_blocking(move || self.build(keys.as_ref()))
            .await
            .map_err(|error| MakerError::BlockingTask(error.to_string()))?
    }

    /// Builds the transaction (payment, withdrawal, own parts, padding to
    /// the plan's shape), encrypts it, and records the maker's own outputs
    /// so they count as incoming until they land.
    fn build<K: ShieldedKeys + ?Sized>(self, keys: &K) -> Result<BuiltTransfer, MakerError> {
        let wallets: Vec<WalletUtxo> = self
            .plan
            .selection
            .inputs
            .iter()
            .map(|input| input.wallet())
            .collect();
        let asset = wallets
            .first()
            .map(|wallet| wallet.utxo.asset)
            .ok_or(TransactionError::NoInputs)?;

        let mut transaction =
            ConfidentialTransaction::new(wallets, self.payer)?.with_output_tree_id(self.tree_id)?;
        if let Some((recipient, amount)) = self.plan.recipient {
            pay_to(&mut transaction, asset, &recipient, amount)?;
        }
        let mut interface_accounts = Vec::new();
        if let Some(target) = self.withdrawal.filter(|_| self.plan.withdrawal > 0) {
            transaction.withdraw(
                asset.asset,
                self.plan.withdrawal,
                target.token_account(&asset.asset),
            )?;
            interface_accounts.push(TransactInterfaceTransferAccounts::SplWithdrawal(
                target.spl_accounts(asset.asset),
            ));
        }
        for amount in &self.plan.own_parts {
            pay_to(&mut transaction, asset, &self.own, *amount)?;
        }
        transaction.pad_utxos(self.plan.shape, &self.own)?;
        let proof_inputs = transaction.encrypt(keys)?;

        let mut expected_outputs = Vec::new();
        for (position, output) in proof_inputs.output_utxos.iter().enumerate() {
            if output.owner_address == Some(self.own) && output.amount > 0 {
                expected_outputs.push(expected_output(
                    &self.own,
                    output,
                    output.hash(self.tree_id)?,
                    self.tree_id,
                    position,
                )?);
            }
        }
        assign_nullifiers(keys, &mut expected_outputs)?;
        Ok(BuiltTransfer {
            proof: ProofWork {
                inputs: proof_inputs,
                interface_accounts,
            },
            expected_outputs,
        })
    }
}

fn pay_to(
    transaction: &mut ConfidentialTransaction,
    asset: Mint,
    recipient: &ShieldedAddress,
    amount: u64,
) -> Result<(), TransactionError> {
    if asset == Mint::SOL {
        transaction.transfer_sol(recipient, amount)?;
    } else {
        transaction.transfer(recipient, asset.asset, amount)?;
    }
    Ok(())
}

/// The wallet UTXO of the maker's own output at `position`, after checking
/// that it opens to `utxo_hash`. Errors with
/// `MakerError::OutputPositionOutOfRange` or a commitment mismatch.
fn expected_output(
    own: &ShieldedAddress,
    output: &SppProofOutputUtxo,
    utxo_hash: [u8; 32],
    tree_id: u16,
    position: usize,
) -> Result<WalletUtxo, MakerError> {
    let slot_index =
        u32::try_from(position).map_err(|_| MakerError::OutputPositionOutOfRange { position })?;
    let utxo = Utxo {
        owner: own.signing_pubkey,
        asset: output.asset,
        amount: output.amount,
        blinding: output.blinding,
        ring_program_id: output.ring_program_id,
        data: output.data.clone(),
    };
    if utxo.hash(&own.nullifier_pubkey, &[0; 32], &[0; 32], tree_id)? != utxo_hash {
        return Err(TransactionError::InputCommitmentMismatch { index: position }.into());
    }
    Ok(WalletUtxo {
        utxo,
        nullifier_pubkey: own.nullifier_pubkey,
        utxo_hash,
        nullifier: [0; 32],
        data_hash: None,
        ring_data_hash: None,
        tree_id,
        leaf_index: 0,
        slot: 0,
        tx_signature: Signature::default(),
        slot_index,
    })
}

/// Fills in each output's nullifier, derived by `keys` in one request.
fn assign_nullifiers<K: ShieldedKeys + ?Sized>(
    keys: &K,
    outputs: &mut [WalletUtxo],
) -> Result<(), MakerError> {
    let requests: Vec<DeriveRequest> = outputs
        .iter()
        .map(|output| DeriveRequest::Nullifier {
            utxo_hash: output.utxo_hash,
            blinding: output.utxo.blinding,
        })
        .collect();
    let nullifiers = keys.derive(&requests)?;
    if nullifiers.len() != outputs.len() {
        return Err(TransactionError::IncompleteDerivation {
            got: nullifiers.len(),
            want: outputs.len(),
        }
        .into());
    }
    for (output, nullifier) in outputs.iter_mut().zip(nullifiers) {
        output.nullifier = nullifier;
    }
    Ok(())
}

/// Everything `schedule_transfer` needs to admit one step.
pub struct TransferStep {
    pub kind: StepKind,
    pub asset: Address,
    pub operation: Option<OperationId>,
    pub plan: TransferPlan,
    pub withdrawal: Option<WithdrawalTarget>,
    pub tail: Option<TailShield>,
    pub vault_before: Option<VaultState>,
    pub fill: Option<FillTransfer>,
}

impl Coordinator {
    /// Builds `transfer`, admits it as a new step (reserving its inputs and
    /// recording its own outputs as incoming) and starts proving it.
    /// Errors with the build error, or with `MakerError::UtxoReserved` /
    /// `MakerError::UtxoNotTracked` when its inputs cannot be reserved.
    pub async fn schedule_transfer(
        &mut self,
        transfer: TransferStep,
    ) -> Result<StepId, MakerError> {
        let TransferStep {
            kind,
            asset,
            operation,
            plan,
            withdrawal,
            tail,
            vault_before,
            fill,
        } = transfer;
        let inputs = plan.selection.hashes();
        let built = TransferBuild {
            plan,
            own: self.identity.own,
            payer: self.identity.payer,
            tree_id: self.identity.tree_id,
            withdrawal,
        }
        .run(self.identity.keys.clone())
        .await?;
        let id = self.steps.next_id();
        let mut step = Step::new(id, kind, built.proof);
        step.asset = Some(asset);
        step.operation = operation;
        step.inputs = inputs;
        step.tail = tail
            .iter()
            .map(|tail| tail.vault_instruction.clone())
            .collect();
        step.tail_shield = tail;
        step.vault_before = vault_before;
        step.fill = fill;
        step.expected_outputs = built.expected_outputs;
        self.admit(step)?;
        Ok(id)
    }

    /// Reserves the step's inputs, records its outputs as incoming, inserts
    /// it and spawns its proof; nothing is recorded if the reservation fails.
    fn admit(&mut self, step: Step) -> Result<(), MakerError> {
        let incoming = step
            .expected_outputs
            .iter()
            .try_fold(0u64, |total, output| total.checked_add(output.utxo.amount))
            .ok_or(MakerError::AmountOverflow {
                context: "step expected outputs",
            })?;
        self.services
            .pending
            .reservations
            .reserve(step.id, &step.inputs)?;
        if let Some(asset) = step.asset {
            self.services.pending.expect(step.id, asset, incoming);
        }
        let id = step.id;
        self.steps.insert(step);
        self.spawn_prove(id);
        Ok(())
    }

    /// Whether UTXOs of `asset` will become selectable without new funds:
    /// some are reserved by a step in flight or not yet indexed.
    pub fn waits_for_utxos(&self, asset: &Address) -> bool {
        let reservations = &self.services.pending.reservations;
        reservations.in_flight(asset) || reservations.unindexed(asset)
    }
}

/// Returns the amounts of the `(utxo_hash, amount)` pairs in `utxos` whose
/// hash is not in `selected`, in iteration order.
pub fn other_amounts<'a>(
    utxos: impl IntoIterator<Item = (&'a [u8; 32], u64)>,
    selected: &BTreeSet<[u8; 32]>,
) -> Vec<u64> {
    utxos
        .into_iter()
        .filter(|(hash, _)| !selected.contains(*hash))
        .map(|(_, amount)| amount)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Own output parts summing above `u64::MAX` fail with `AmountOverflow`
    /// instead of wrapping.
    #[test]
    fn plan_consolidate_rejects_overflowing_own_parts() {
        let selection = Selection {
            inputs: Vec::new(),
            total: 1,
        };
        let refused = plan_consolidate(selection, 0, vec![u64::MAX, 2]).err();
        assert!(
            matches!(
                refused,
                Some(MakerError::AmountOverflow {
                    context: "transfer own parts"
                })
            ),
            "got {refused:?}, want AmountOverflow {{ context: \"transfer own parts\" }}"
        );
    }
}
