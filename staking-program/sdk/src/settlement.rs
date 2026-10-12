//! Crank-side settlement: the batch entries, both SPP legs, the settlement
//! proof witness, and the `execute_batch` instruction for one hash chain.
//!
//! Flow: [`SettlementBuilder::new`] fixes every blinding from the two seeds,
//! [`create_batch_data`](SettlementBuilder::create_batch_data) and
//! [`batch_entries`](SettlementBuilder::batch_entries) fill the batch, the
//! [`withdrawal_request`](SettlementBuilder::withdrawal_request) and
//! [`payout_request`](SettlementBuilder::payout_request) SPP proofs are made,
//! [`settlement_inputs`](SettlementBuilder::settlement_inputs) checks them
//! against the program's leg builders and returns the settlement witness, and
//! [`execute_batch`](SettlementBuilder::execute_batch) assembles the call.
//! An all-cancelled chain has no SPP legs: prove
//! [`proof_inputs`](SettlementBuilder::proof_inputs) directly.
use anyhow::{anyhow, bail, ensure, Result};
use p256::{elliptic_curve::sec1::ToEncodedPoint, PublicKey as P256PublicKey, SecretKey};
use private_staking_program::{
    instructions::{execute::ExecuteBatchIxData, management::CreateBatchIxData, proof::LogicProof},
    math::PoolSnapshot,
    pda, pool,
    settlement::{
        self, CachedUtxo, EntryKind, LegContext, SettlementAmounts, SettlementLegs,
        SettlementProofContext, SppProofData, PAYOUT_CIRCUIT, WITHDRAWAL_CIRCUIT,
    },
    state::{BatchEntry, Config, RecipientAddress},
    N,
};
use private_staking_prover::{CircuitId as LogicCircuit, ProofInputMap, PROVER};
use solana_address::Address;
use solana_instruction::Instruction;
use zolana_client::{NonInclusionProof, SpendProof, NULLIFIER_TREE_HEIGHT};
use zolana_hasher::{
    primitives::{hash_bytes, right_align, solana_owner_identity},
    Hasher, Poseidon,
};
use zolana_interface::instruction::instruction_data::transact::{
    CacheAccess, CircuitId, InputUtxo, OwnerTag, TransactIxData, TransactOutput, TreeContext,
};
use zolana_keypair::{NullifierKey, P256Pubkey, PublicKey, ShieldedAddress};
use zolana_program::{
    derive_output_blinding_seed, derive_private_tx_blinding, derive_transact_output_blinding,
    PrivateTxHash,
};
use zolana_transaction::{
    instructions::transact::{CacheAccounts, SettlementTransfer, SppProofInputs},
    utxo::SppProofInputUtxo,
    ExternalData, Mint, SppProofOutputUtxo, Utxo,
};

use crate::{
    allocation::{Allocation, IntentAmount},
    encryption::{self, Context},
    instructions::{self, ExecuteBatchAccounts, ExecuteSppAccounts, PoolAccounts},
    intent::{bytes, scalar},
    note::{Note, PayoutNote, PAYOUT_ASSET_POOL, PAYOUT_ASSET_SOL},
    spp::{SppProofRequest, SppTransaction},
};

/// An escrow note with the leaf index of its commitment in the escrow tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Escrow {
    pub note: Note,
    pub leaf_index: u64,
    /// `Some` iff the escrow was cancelled (its allocation is
    /// [`IntentAmount::Cancelled`]); says how its spend is proven.
    pub cancelled: Option<CancelledProof>,
}

/// How a cancelled escrow's spend is shown to `execute_batch`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CancelledProof {
    /// The nullifier PDA still exists; the program checks it.
    Queued,
    /// The nullifier was absorbed into the SPP nullifier tree (its PDA is
    /// closed); the settlement proof shows the inclusion of its indexed leaf
    /// under [`SettlementParams::nullifier_root`].
    InTree {
        /// The leaf's next value in the indexed nullifier tree.
        next_value: [u8; 32],
        /// `NULLIFIER_TREE_HEIGHT` siblings, leaf to root.
        path: Vec<[u8; 32]>,
        leaf_index: u64,
    },
}

/// Inclusion of a cancelled escrow's nullifier in the nullifier tree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NullifierInclusion {
    pub next_value: [u8; 32],
    pub path: [[u8; 32]; NULLIFIER_TREE_HEIGHT],
    pub leaf_index: u64,
}

impl CancelledProof {
    fn kind(&self) -> EntryKind {
        match self {
            Self::Queued => EntryKind::QueuedCancellation,
            Self::InTree { .. } => EntryKind::AbsorbedCancellation,
        }
    }

    fn inclusion(&self) -> Result<Option<NullifierInclusion>> {
        match self {
            Self::Queued => Ok(None),
            Self::InTree {
                next_value,
                path,
                leaf_index,
            } => Ok(Some(NullifierInclusion {
                next_value: *next_value,
                path: path.as_slice().try_into().map_err(|_| {
                    anyhow!("a nullifier path has {NULLIFIER_TREE_HEIGHT} siblings")
                })?,
                leaf_index: *leaf_index,
            })),
        }
    }
}

pub struct SettlementParams<'a> {
    pub config_key: Address,
    pub config: &'a Config,
    pub epoch: Address,
    pub settlement_deadline: i64,
    pub hash_chain_index: u16,
    /// Raw id of the escrow tree, which also receives the cached UTXO and payouts.
    pub tree_id: u16,
    /// Pays `execute_batch` and SPP fees, and sponsors the cache.
    pub payer: Address,
    /// Paid the batch rent in `create_batch` and receives it back.
    pub rent_payer: Address,
    /// Every chain entry in chain order, each carrying its intent data.
    pub escrows: Vec<Escrow>,
    pub allocation: &'a Allocation,
    pub pool: PoolSnapshot,
    pub pool_asset_id: u64,
    /// Root of the escrow tree's nullifier tree that every
    /// [`CancelledProof::InTree`] path leads to; bound into the batch hash.
    pub nullifier_root: [u8; 32],
    /// Index of `nullifier_root` in the escrow tree's nullifier root history.
    pub nullifier_root_index: u16,
    pub ephemeral: SecretKey,
    pub salt: [u8; 16],
    /// Root seed of the withdrawal leg (escrows -> cached UTXO).
    pub withdrawal_blinding_seed: [u8; 32],
    /// Root seed of the payout leg (cached UTXO -> payouts).
    pub payout_blinding_seed: [u8; 32],
}

#[derive(Clone, Debug)]
struct Payout {
    note: PayoutNote,
    hash: [u8; 32],
    output: TransactOutput,
    view_tag: [u8; 32],
}

#[derive(Clone, Debug)]
struct Entry {
    escrow: Escrow,
    recipient: RecipientAddress,
    hash: [u8; 32],
    nullifier: [u8; 32],
    /// `Some` iff cancelled and absorbed into the nullifier tree.
    inclusion: Option<NullifierInclusion>,
    payout: Option<Payout>,
}

impl Entry {
    fn kind(&self) -> EntryKind {
        match (&self.payout, &self.escrow.cancelled) {
            (Some(_), _) => EntryKind::Active,
            (None, Some(cancelled)) => cancelled.kind(),
            // `new` rejects a cancelled escrow without a proof kind.
            (None, None) => EntryKind::QueuedCancellation,
        }
    }
}

/// The cached UTXO with the seeds that fixed its blinding.
#[derive(Clone, Copy, Debug)]
struct Legs {
    cached: CachedUtxo,
    withdrawal_output_seed: [u8; 32],
    withdrawal_tx_blinding: [u8; 32],
    payout_output_seed: [u8; 32],
    payout_tx_blinding: [u8; 32],
}

pub struct SettlementBuilder {
    config_key: Address,
    config: Config,
    epoch: Address,
    settlement_deadline: i64,
    hash_chain_index: u16,
    tree_id: u16,
    payer: Address,
    rent_payer: Address,
    entries: Vec<Entry>,
    amounts: SettlementAmounts,
    pool: PoolSnapshot,
    pool_asset_id: u64,
    nullifier_root: [u8; 32],
    nullifier_root_index: u16,
    ephemeral: SecretKey,
    tx_viewing_pk: [u8; 33],
    salt: [u8; 16],
    withdrawal_blinding_seed: [u8; 32],
    payout_blinding_seed: [u8; 32],
    legs: Option<Legs>,
}

/// One circuit slot (`Escrow{i}_*`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SettlementSlot {
    pub present: bool,
    pub cancelled: bool,
    /// `Some` for a cancellation absorbed into the nullifier tree.
    pub nullifier_inclusion: Option<NullifierInclusion>,
    pub pool_asset: bool,
    pub amount: u64,
    pub blinding: [u8; 32],
    pub nullifier_pk: [u8; 32],
    pub payout_blinding: [u8; 32],
    pub signing_pk: [u8; 34],
    /// Compressed SEC1 recipient viewing key.
    pub viewing_pk: [u8; 33],
}

pub struct SettlementProofInputs {
    pub context: SettlementProofContext,
    pub pool: PoolSnapshot,
    pub amounts: SettlementAmounts,
    /// The nullifier root `context.batch_hash` binds.
    pub nullifier_root: [u8; 32],
    /// The stored entries in chain order (at most `N`); the witness pads the
    /// rest with copies of the first.
    pub slots: Vec<SettlementSlot>,
    pub ephemeral: SecretKey,
    pub crank_viewing_pk: [u8; 33],
    pub salt: [u8; 16],
    pub withdrawal_tx_blinding: [u8; 32],
    pub payout_tx_blinding: [u8; 32],
}

fn zero_nullifier_key() -> NullifierKey {
    NullifierKey::from_secret([0; 31])
}

fn cache_access(circuit: CircuitId) -> Result<CacheAccess> {
    match circuit {
        CircuitId::ConfidentialEddsaCached(_, _, _, access) => Ok(access),
        _ => bail!("settlement circuit without cache access"),
    }
}

fn hashing<E: std::fmt::Debug>(error: E) -> anyhow::Error {
    anyhow!("hashing failed: {error:?}")
}

fn compressed(key: &P256PublicKey) -> Result<[u8; 33]> {
    Ok(key.to_encoded_point(true).as_bytes().try_into()?)
}

fn uncompressed(key: &[u8; 33]) -> Result<Vec<u8>> {
    Ok(P256PublicKey::from_sec1_bytes(key)?
        .to_encoded_point(false)
        .as_bytes()
        .to_vec())
}

fn index_u32(index: usize) -> Result<u32> {
    Ok(u32::try_from(index)?)
}

impl SettlementBuilder {
    pub fn new(params: SettlementParams<'_>) -> Result<Self> {
        let SettlementParams {
            config_key,
            config,
            epoch,
            settlement_deadline,
            hash_chain_index,
            tree_id,
            payer,
            rent_payer,
            escrows,
            allocation,
            pool,
            pool_asset_id,
            nullifier_root,
            nullifier_root_index,
            ephemeral,
            salt,
            withdrawal_blinding_seed,
            payout_blinding_seed,
        } = params;
        ensure!(
            !escrows.is_empty() && escrows.len() <= N,
            "a batch holds 1..={N} entries"
        );
        ensure!(
            allocation.utxos.len() == escrows.len(),
            "allocation does not cover every escrow"
        );
        ensure!(settlement_deadline > 0, "invalid settlement deadline");
        let authority = PublicKey::from_pda(&pda::escrow_authority(&config_key).0);
        let zero_key = zero_nullifier_key();
        let zero_pk = zero_key.pubkey()?;
        let pool_mint = Mint::new(config.pool_mint, pool_asset_id);

        // Escrow commitments and nullifiers in chain order.
        let mut entries = Vec::with_capacity(escrows.len());
        for (index, (escrow, (intent, _))) in escrows.into_iter().zip(&allocation.utxos).enumerate()
        {
            let note = &escrow.note;
            let data = note
                .intent
                .ok_or_else(|| anyhow!("escrow {index} has no intent data"))?;
            ensure!(data.epoch == epoch, "escrow {index}: wrong epoch");
            ensure!(note.tree_id == tree_id, "escrow {index}: wrong tree");
            ensure!(
                note.signing_pubkey == *authority.as_bytes() && note.nullifier_pubkey == zero_pk,
                "escrow {index} is not owned by the escrow authority"
            );
            ensure!(
                matches!(data.recipient.signing_pubkey.first(), Some(1 | 2)),
                "escrow {index}: recipient needs a Solana signing key"
            );
            let asset = Mint::new(note.asset, note.asset_id);
            ensure!(
                (*intent == IntentAmount::Cancelled) == escrow.cancelled.is_some(),
                "escrow {index}: a cancelled proof is required exactly for cancelled allocations"
            );
            let inclusion = match &escrow.cancelled {
                Some(cancelled) => cancelled
                    .inclusion()
                    .map_err(|error| anyhow!("escrow {index}: {error}"))?,
                None => None,
            };
            match *intent {
                IntentAmount::Deposit(amount) => ensure!(
                    asset == Mint::SOL && note.amount == amount,
                    "escrow {index} is not a {amount} lamport deposit"
                ),
                IntentAmount::Withdrawal(amount) => ensure!(
                    asset == pool_mint && note.amount == amount,
                    "escrow {index} is not a {amount} pool token withdrawal"
                ),
                IntentAmount::Cancelled => ensure!(
                    asset == Mint::SOL || asset == pool_mint,
                    "escrow {index} has an unknown asset"
                ),
            }
            let hash = note.hash()?;
            let nullifier = zero_key.nullifier(&hash, &note.blinding)?;
            entries.push(Entry {
                recipient: data.recipient,
                escrow,
                hash,
                nullifier,
                inclusion,
                payout: None,
            });
        }

        let tx_viewing_pk = compressed(&ephemeral.public_key())?;
        let first_active = entries
            .iter()
            .zip(&allocation.utxos)
            .find(|(_, (intent, _))| *intent != IntentAmount::Cancelled)
            .map(|(entry, _)| entry.nullifier);
        let legs = match first_active {
            Some(first) => {
                allocation
                    .amounts
                    .pool_operation(&pool)
                    .map_err(|error| anyhow!("allocation does not match the pool: {error:?}"))?;
                let sol_vault = pda::sol_vault(&config_key).0;
                let withdrawal_output_seed =
                    derive_output_blinding_seed(&first, &withdrawal_blinding_seed)?;
                let cached =
                    settlement::cached_utxo(&sol_vault, &first, &withdrawal_output_seed, tree_id)
                        .map_err(hashing)?;
                Some(Legs {
                    cached,
                    withdrawal_output_seed,
                    withdrawal_tx_blinding: derive_private_tx_blinding(
                        &first,
                        &withdrawal_blinding_seed,
                    )?,
                    payout_output_seed: derive_output_blinding_seed(
                        &cached.nullifier,
                        &payout_blinding_seed,
                    )?,
                    payout_tx_blinding: derive_private_tx_blinding(
                        &cached.nullifier,
                        &payout_blinding_seed,
                    )?,
                })
            }
            None => {
                ensure!(
                    allocation.amounts == SettlementAmounts::default(),
                    "an all-cancelled chain moves no funds"
                );
                None
            }
        };

        // Payouts: the opposite asset, in active order.
        let mut active = 0usize;
        for (index, (entry, (intent, payout))) in
            entries.iter_mut().zip(&allocation.utxos).enumerate()
        {
            let asset = match intent {
                IntentAmount::Deposit(_) => PAYOUT_ASSET_POOL,
                IntentAmount::Withdrawal(_) => PAYOUT_ASSET_SOL,
                IntentAmount::Cancelled => {
                    ensure!(payout.is_none(), "cancelled escrow {index} has a payout");
                    continue;
                }
            };
            let amount = payout.ok_or_else(|| anyhow!("escrow {index} has no payout"))?;
            let legs = legs
                .as_ref()
                .ok_or_else(|| anyhow!("active entry without legs"))?;
            let output_index = index_u32(active)?;
            let note = PayoutNote {
                version: 1,
                tree_id,
                asset,
                amount,
                blinding: derive_transact_output_blinding(
                    &legs.cached.nullifier,
                    &legs.payout_output_seed,
                    output_index,
                )?,
            };
            let hash = note.hash(&entry.recipient, config.pool_mint, pool_asset_id)?;
            let view_tag =
                PublicKey::from_bytes(entry.recipient.signing_pubkey)?.confidential_view_tag()?;
            let data = encryption::encrypt_payout(
                &note,
                &entry.recipient,
                config.pool_mint,
                pool_asset_id,
                &ephemeral,
                &config.crank.viewing_pubkey,
                &Context {
                    tx_viewing_pk,
                    salt,
                    output_index,
                    utxo_hash: hash,
                },
            )?;
            entry.payout = Some(Payout {
                note,
                hash,
                output: TransactOutput {
                    utxo_hash: hash,
                    owner_tag: OwnerTag::Inline(view_tag),
                    data: Some(data),
                },
                view_tag,
            });
            active += 1;
        }

        Ok(Self {
            config_key,
            config: *config,
            epoch,
            settlement_deadline,
            hash_chain_index,
            tree_id,
            payer,
            rent_payer,
            entries,
            amounts: allocation.amounts,
            pool,
            pool_asset_id,
            nullifier_root,
            nullifier_root_index,
            ephemeral,
            tx_viewing_pk,
            salt,
            withdrawal_blinding_seed,
            payout_blinding_seed,
            legs,
        })
    }

    fn sol_vault(&self) -> Address {
        pda::sol_vault(&self.config_key).0
    }

    fn token_vault(&self) -> Address {
        pool::token_vault(&self.config_key, &self.config)
    }

    fn escrow_authority(&self) -> Address {
        pda::escrow_authority(&self.config_key).0
    }

    /// The write cache of the withdrawal leg, read by the payout leg.
    pub fn cache(&self) -> Address {
        zolana_interface::pda::cache(&self.payer, u64::from(self.hash_chain_index)).0
    }

    fn legs(&self) -> Result<&Legs> {
        self.legs
            .as_ref()
            .ok_or_else(|| anyhow!("every entry is cancelled; no SPP legs"))
    }

    fn active(&self) -> impl Iterator<Item = (&Entry, &Payout)> {
        self.entries
            .iter()
            .filter_map(|entry| entry.payout.as_ref().map(|payout| (entry, payout)))
    }

    pub fn active_count(&self) -> usize {
        self.active().count()
    }

    pub fn tx_viewing_pk(&self) -> [u8; 33] {
        self.tx_viewing_pk
    }

    /// The SOL UTXO the withdrawal leg caches; `None` when every entry is cancelled.
    pub fn cached_utxo(&self) -> Option<CachedUtxo> {
        self.legs.map(|legs| legs.cached)
    }

    pub fn withdrawal_output_seed(&self) -> Option<[u8; 32]> {
        self.legs.map(|legs| legs.withdrawal_output_seed)
    }

    /// The escrow nullifiers in chain order.
    pub fn nullifiers(&self) -> Vec<[u8; 32]> {
        self.entries.iter().map(|entry| entry.nullifier).collect()
    }

    /// `(utxo_hash, nullifier, leaf_index)` of each active escrow in chain
    /// order: the spend proofs [`withdrawal_request`](Self::withdrawal_request) takes.
    pub fn spend_targets(&self) -> Vec<([u8; 32], [u8; 32], u64)> {
        self.active()
            .map(|(entry, _)| (entry.hash, entry.nullifier, entry.escrow.leaf_index))
            .collect()
    }

    /// Nullifiers of the withdrawal leg's compact input padding, one per
    /// padding non-inclusion proof.
    pub fn withdrawal_padding_nullifiers(&self) -> Result<Vec<[u8; 32]>> {
        let padding = SppProofInputUtxo::compact(self.tree_id)?.nullifier;
        Ok(vec![
            padding;
            N.checked_sub(self.active_count())
                .ok_or_else(|| anyhow!("too many active entries"))?
        ])
    }

    /// Payout notes in chain order; `None` for cancelled entries.
    pub fn payout_notes(&self) -> Vec<Option<PayoutNote>> {
        self.entries
            .iter()
            .map(|entry| entry.payout.as_ref().map(|payout| payout.note))
            .collect()
    }

    /// `Poseidon(chain, escrow_hash)` folded from zero, as stored by the epoch.
    pub fn utxo_hash_chain(&self) -> Result<[u8; 32]> {
        self.entries.iter().try_fold([0; 32], |chain, entry| {
            Poseidon::hashv(&[&chain, &entry.hash]).map_err(hashing)
        })
    }

    pub fn create_batch_data(&self) -> CreateBatchIxData {
        CreateBatchIxData {
            hash_chain_index: self.hash_chain_index,
            tx_viewing_pk: self.tx_viewing_pk,
            salt: self.salt,
        }
    }

    /// How each entry resolves, in chain order; aligned with
    /// [`batch_entries`](Self::batch_entries).
    pub fn entry_kinds(&self) -> Vec<EntryKind> {
        self.entries.iter().map(Entry::kind).collect()
    }

    pub fn nullifier_root(&self) -> [u8; 32] {
        self.nullifier_root
    }

    /// `fill_batch` entries in chain order.
    pub fn batch_entries(&self) -> Vec<BatchEntry> {
        self.entries
            .iter()
            .map(|entry| {
                (
                    InputUtxo {
                        nullifier_hash: entry.nullifier,
                        tree_index: 0,
                    },
                    entry.payout.as_ref().map(|payout| payout.output.clone()),
                )
            })
            .collect()
    }

    fn cached_output(&self, legs: &Legs) -> Result<SppProofOutputUtxo> {
        let sol_vault = self.sol_vault();
        let mut output = SppProofOutputUtxo::new(
            Mint::SOL,
            0,
            ShieldedAddress {
                signing_pubkey: PublicKey::from_pda(&sol_vault),
                nullifier_pubkey: zero_nullifier_key().pubkey()?,
                viewing_pubkey: P256Pubkey::from_bytes(self.config.crank.viewing_pubkey)?,
            },
        )?
        .with_cache_slot(0)?;
        output.blinding = legs.cached.blinding;
        ensure!(
            output.hash(self.tree_id)? == legs.cached.hash,
            "cached UTXO hash mismatch"
        );
        Ok(output)
    }

    fn cached_transact_output(&self, legs: &Legs) -> TransactOutput {
        TransactOutput {
            utxo_hash: legs.cached.hash,
            owner_tag: OwnerTag::Inline(self.sol_vault().to_bytes()),
            data: None,
        }
    }

    fn compact_outputs(
        &self,
        outputs: &mut Vec<SppProofOutputUtxo>,
        first_nullifier: &[u8; 32],
        output_seed: &[u8; 32],
        width: usize,
    ) -> Result<()> {
        for index in outputs.len()..width {
            outputs.push(SppProofOutputUtxo {
                blinding: derive_transact_output_blinding(
                    first_nullifier,
                    output_seed,
                    index_u32(index)?,
                )?,
                compact: true,
                ..Default::default()
            });
        }
        Ok(())
    }

    fn external_data(
        &self,
        outputs: Vec<TransactOutput>,
        tags: Vec<[u8; 32]>,
        transfers: [(u64, SettlementTransfer); 2],
    ) -> Result<ExternalData> {
        let mut external = ExternalData::new(self.tx_viewing_pk, self.salt, outputs, tags, vec![])
            .with_interface_transfers(
                transfers
                    .into_iter()
                    .filter(|(amount, _)| *amount != 0)
                    .map(|(_, transfer)| transfer)
                    .collect(),
            )?;
        external.expiry_unix_ts = u64::try_from(self.settlement_deadline)?;
        Ok(external)
    }

    /// Withdrawal leg (shape 16x2): the active escrows, compact padded, into
    /// the vaults, writing the cached UTXO to cache slot 0. `spend_proofs`
    /// follow the active escrows in chain order; `padding` holds one
    /// non-inclusion proof per compact input slot.
    pub fn withdrawal_request(
        &self,
        spend_proofs: Vec<SpendProof>,
        padding: Vec<NonInclusionProof>,
    ) -> Result<SppProofRequest> {
        let legs = self.legs()?;
        let zero_pk = zero_nullifier_key().pubkey()?;
        let mut inputs = Vec::with_capacity(N);
        for (entry, _) in self.active() {
            let note = &entry.escrow.note;
            inputs.push(SppProofInputUtxo {
                utxo: note.utxo()?,
                nullifier_pubkey: zero_pk,
                utxo_hash: entry.hash,
                nullifier: entry.nullifier,
                data_hash: Some(note.data_hash()?),
                ring_data_hash: None,
                tree_id: self.tree_id,
                leaf_index: entry.escrow.leaf_index,
                cache_slot: None,
                compact: false,
            });
        }
        while inputs.len() < N {
            inputs.push(SppProofInputUtxo::compact(self.tree_id)?);
        }
        let first = self
            .active()
            .next()
            .map(|(entry, _)| entry.nullifier)
            .ok_or_else(|| anyhow!("no active entry"))?;
        let mut outputs = vec![self.cached_output(legs)?];
        self.compact_outputs(&mut outputs, &first, &legs.withdrawal_output_seed, 2)?;
        let sol_vault = self.sol_vault();
        let external = self.external_data(
            vec![self.cached_transact_output(legs)],
            vec![sol_vault.to_bytes()],
            [
                (
                    self.amounts.escrow_sol,
                    SettlementTransfer::Sol {
                        is_deposit: false,
                        amount: self.amounts.escrow_sol,
                        user_sol_account: sol_vault,
                    },
                ),
                (
                    self.amounts.escrow_pool_tokens,
                    SettlementTransfer::Spl {
                        mint: self.config.pool_mint,
                        is_deposit: false,
                        amount: self.amounts.escrow_pool_tokens,
                        user_spl_token: self.token_vault(),
                    },
                ),
            ],
        )?;
        Ok(SppProofRequest {
            inputs: SppProofInputs {
                input_utxos: inputs,
                output_utxos: outputs,
                blinding_seed: self.withdrawal_blinding_seed,
                output_tree_id: self.tree_id,
                external_data: external,
                payer: self.payer,
                cache_accounts: CacheAccounts {
                    read: None,
                    write: Some(self.cache()),
                },
            },
            spend_proofs,
            padding_proofs: padding,
            additional_signers: vec![self.escrow_authority()],
            expected_cache_access: Some(cache_access(WITHDRAWAL_CIRCUIT)?),
        })
    }

    /// Payout leg (shape 1x16): the cached UTXO read from cache slot 0 into
    /// the stored payouts, compact padded, funded from the vaults.
    /// `cached_non_inclusion` proves the cached nullifier is unspent.
    pub fn payout_request(
        &self,
        cached_non_inclusion: NonInclusionProof,
    ) -> Result<SppProofRequest> {
        let legs = self.legs()?;
        let sol_vault = self.sol_vault();
        let input = SppProofInputUtxo {
            utxo: Utxo {
                owner: PublicKey::from_pda(&sol_vault),
                asset: Mint::SOL,
                amount: 0,
                blinding: legs.cached.blinding,
                ring_program_id: None,
                data: Default::default(),
            },
            nullifier_pubkey: zero_nullifier_key().pubkey()?,
            utxo_hash: legs.cached.hash,
            nullifier: legs.cached.nullifier,
            data_hash: None,
            ring_data_hash: None,
            tree_id: self.tree_id,
            leaf_index: 0,
            cache_slot: None,
            compact: false,
        }
        .with_cache_slot(0)?;
        let pool_mint = Mint::new(self.config.pool_mint, self.pool_asset_id);
        let mut outputs = Vec::with_capacity(N);
        let mut transact_outputs = Vec::with_capacity(N);
        let mut tags = Vec::with_capacity(N);
        for (entry, payout) in self.active() {
            let asset = if payout.note.asset == PAYOUT_ASSET_SOL {
                Mint::SOL
            } else {
                pool_mint
            };
            let mut output = SppProofOutputUtxo::new(
                asset,
                payout.note.amount,
                ShieldedAddress {
                    signing_pubkey: PublicKey::from_bytes(entry.recipient.signing_pubkey)?,
                    nullifier_pubkey: entry.recipient.nullifier_pubkey,
                    viewing_pubkey: P256Pubkey::from_bytes(entry.recipient.viewing_pubkey)?,
                },
            )?;
            output.blinding = payout.note.blinding;
            ensure!(
                output.hash(self.tree_id)? == payout.hash,
                "payout UTXO hash mismatch"
            );
            outputs.push(output);
            transact_outputs.push(payout.output.clone());
            tags.push(payout.view_tag);
        }
        self.compact_outputs(
            &mut outputs,
            &legs.cached.nullifier,
            &legs.payout_output_seed,
            N,
        )?;
        let external = self.external_data(
            transact_outputs,
            tags,
            [
                (
                    self.amounts.payout_sol,
                    SettlementTransfer::Sol {
                        is_deposit: true,
                        amount: self.amounts.payout_sol,
                        user_sol_account: sol_vault,
                    },
                ),
                (
                    self.amounts.payout_pool_tokens,
                    SettlementTransfer::Spl {
                        mint: self.config.pool_mint,
                        is_deposit: true,
                        amount: self.amounts.payout_pool_tokens,
                        user_spl_token: self.token_vault(),
                    },
                ),
            ],
        )?;
        Ok(SppProofRequest {
            inputs: SppProofInputs {
                input_utxos: vec![input],
                output_utxos: outputs,
                blinding_seed: self.payout_blinding_seed,
                output_tree_id: self.tree_id,
                external_data: external,
                payer: self.payer,
                cache_accounts: CacheAccounts {
                    read: Some(self.cache()),
                    write: None,
                },
            },
            spend_proofs: vec![],
            padding_proofs: vec![cached_non_inclusion],
            additional_signers: vec![sol_vault],
            expected_cache_access: Some(cache_access(PAYOUT_CIRCUIT)?),
        })
    }

    /// `private_tx_hash` of the withdrawal leg over its non-padding hashes.
    pub fn spend_private_tx_hash(&self) -> Result<[u8; 32]> {
        let Some(legs) = &self.legs else {
            return Ok([0; 32]);
        };
        let inputs: Vec<[u8; 32]> = self.active().map(|(entry, _)| entry.hash).collect();
        Ok(
            PrivateTxHash::new(&inputs, &[legs.cached.hash], &legs.withdrawal_tx_blinding)
                .hash()?,
        )
    }

    /// `private_tx_hash` of the payout leg over its non-padding hashes.
    pub fn payout_private_tx_hash(&self) -> Result<[u8; 32]> {
        let Some(legs) = &self.legs else {
            return Ok([0; 32]);
        };
        let outputs: Vec<[u8; 32]> = self.active().map(|(_, payout)| payout.hash).collect();
        Ok(PrivateTxHash::new(&[legs.cached.hash], &outputs, &legs.payout_tx_blinding).hash()?)
    }

    fn leg_context<'a>(
        &self,
        spl_interface_bump: u8,
        sol_vault: &'a Address,
    ) -> Result<LegContext<'a>> {
        Ok(LegContext {
            expiry_unix_ts: u64::try_from(self.settlement_deadline)?,
            tx_viewing_pk: self.tx_viewing_pk,
            salt: self.salt,
            spl_interface_bump,
            sol_vault,
        })
    }

    /// The program's view of both legs, from the proven SPP transactions.
    fn expected_legs(
        &self,
        spend: &SppTransaction,
        payout: &SppTransaction,
        spl_interface_bump: u8,
    ) -> Result<(TransactIxData, TransactIxData)> {
        let legs = self.legs()?;
        let sol_vault = self.sol_vault();
        let context = self.leg_context(spl_interface_bump, &sol_vault)?;
        let active_inputs: Vec<InputUtxo> = self
            .batch_entries()
            .into_iter()
            .filter(|(_, output)| output.is_some())
            .map(|(input, _)| input)
            .collect();
        let outputs: Vec<TransactOutput> = self
            .active()
            .map(|(_, payout)| payout.output.clone())
            .collect();
        let tree_context = single_tree_context(&spend.data)?;
        let payout_context = single_tree_context(&payout.data)?;
        let withdrawal = settlement::withdrawal_ix_data(
            &context,
            &active_inputs,
            &legs.cached,
            &self.amounts,
            tree_context,
            &proof_data(&spend.data),
        )
        .map_err(|error| anyhow!("withdrawal_ix_data: {error:?}"))?;
        let payout_data = settlement::payout_ix_data(
            &context,
            outputs,
            &legs.cached,
            &self.amounts,
            payout_context.nullifier_tree_root_index,
            &proof_data(&payout.data),
        )
        .map_err(|error| anyhow!("payout_ix_data: {error:?}"))?;
        Ok((withdrawal, payout_data))
    }

    /// Checks both proven legs field by field against the program's builders
    /// and returns the settlement proof witness.
    pub fn settlement_inputs(
        &self,
        spend: &SppTransaction,
        payout: &SppTransaction,
        spl_interface_bump: u8,
    ) -> Result<SettlementProofInputs> {
        let (withdrawal, payout_data) = self.expected_legs(spend, payout, spl_interface_bump)?;
        compare_ix("withdrawal", &withdrawal, &spend.data)?;
        compare_ix("payout", &payout_data, &payout.data)?;
        ensure!(
            spend.data.private_tx_hash == self.spend_private_tx_hash()?,
            "withdrawal private_tx_hash differs from the settlement witness"
        );
        ensure!(
            payout.data.private_tx_hash == self.payout_private_tx_hash()?,
            "payout private_tx_hash differs from the settlement witness"
        );
        let tree = vec![self.config.escrow_tree];
        ensure!(
            spend.input_trees == tree && payout.input_trees == tree,
            "both legs must spend from the escrow tree"
        );
        ensure!(
            spend.cache_access == Some(cache_access(WITHDRAWAL_CIRCUIT)?)
                && payout.cache_access == Some(cache_access(PAYOUT_CIRCUIT)?),
            "unexpected cache access"
        );
        self.proof_inputs()
    }

    /// The settlement witness; needs no SPP proof (both private transaction
    /// hashes are computed locally).
    pub fn proof_inputs(&self) -> Result<SettlementProofInputs> {
        let entries_hash = settlement::entries_hash(
            &self.batch_entries(),
            &self.entry_kinds(),
            &self.tx_viewing_pk,
            &self.salt,
        )
        .map_err(hashing)?;
        let authority = self.escrow_authority();
        let context = SettlementProofContext {
            spend_private_tx_hash: self.spend_private_tx_hash()?,
            payout_private_tx_hash: self.payout_private_tx_hash()?,
            cached_utxo_hash: self.legs.map_or([0; 32], |legs| legs.cached.hash),
            utxo_hash_chain: self.utxo_hash_chain()?,
            intent_count: u16::try_from(self.entries.len())?,
            epoch_hash: hash_bytes(self.epoch.as_array())?,
            pool_mint_hash: hash_bytes(self.config.pool_mint.as_array())?,
            crank_viewing_hash: hash_bytes(&self.config.crank.viewing_pubkey)?,
            escrow_owner_identity: solana_owner_identity(authority.as_array())?,
            tree_id: self.tree_id,
            pool_hash: settlement::pool_hash(&self.pool).map_err(hashing)?,
            batch_hash: settlement::batch_hash(&self.amounts, &entries_hash, &self.nullifier_root)
                .map_err(hashing)?,
        };
        let slots = self
            .entries
            .iter()
            .map(|entry| {
                let note = &entry.escrow.note;
                SettlementSlot {
                    present: true,
                    cancelled: entry.payout.is_none(),
                    nullifier_inclusion: entry.inclusion,
                    pool_asset: note.asset == self.config.pool_mint,
                    amount: note.amount,
                    blinding: note.blinding,
                    nullifier_pk: entry.recipient.nullifier_pubkey,
                    payout_blinding: entry
                        .payout
                        .as_ref()
                        .map_or([0; 32], |payout| payout.note.blinding),
                    signing_pk: entry.recipient.signing_pubkey,
                    viewing_pk: entry.recipient.viewing_pubkey,
                }
            })
            .collect();
        Ok(SettlementProofInputs {
            context,
            pool: self.pool,
            amounts: self.amounts,
            nullifier_root: self.nullifier_root,
            slots,
            ephemeral: self.ephemeral.clone(),
            crank_viewing_pk: self.config.crank.viewing_pubkey,
            salt: self.salt,
            withdrawal_tx_blinding: self
                .legs
                .map_or([0; 32], |legs| legs.withdrawal_tx_blinding),
            payout_tx_blinding: self.legs.map_or([0; 32], |legs| legs.payout_tx_blinding),
        })
    }

    /// `execute_batch` with its full account list. `legs` are the proven
    /// withdrawal and payout transactions, `None` iff every entry is cancelled.
    pub fn execute_batch(
        &self,
        proof: LogicProof,
        legs: Option<(SppTransaction, SppTransaction)>,
        pool: &PoolAccounts,
    ) -> Result<Instruction> {
        ensure!(
            legs.is_some() == self.legs.is_some(),
            "legs must be present iff an entry settles"
        );
        let settlement_legs = match &legs {
            Some((spend, payout)) => Some(SettlementLegs {
                amounts: self.amounts,
                escrow_tree_context: single_tree_context(&spend.data)?,
                payout_nullifier_root_index: single_tree_context(&payout.data)?
                    .nullifier_tree_root_index,
                withdrawal_output_seed: self.legs()?.withdrawal_output_seed,
                spend: proof_data(&spend.data),
                payout: proof_data(&payout.data),
            }),
            None => None,
        };
        let escrow_tree = self.config.escrow_tree;
        // Cancelled entries, queued or absorbed, pass the canonical nullifier
        // PDA read-only; the program tells them apart by the account's owner.
        let nullifier_pdas = self
            .entries
            .iter()
            .map(|entry| {
                (
                    zolana_interface::pda::nullifier_pda(&escrow_tree, &entry.nullifier).0,
                    entry.payout.is_some(),
                )
            })
            .collect();
        let spp = self.legs.map(|legs| ExecuteSppAccounts {
            cache: self.cache(),
            cached_nullifier_pda: zolana_interface::pda::nullifier_pda(
                &escrow_tree,
                &legs.cached.nullifier,
            )
            .0,
            spl_interface: zolana_interface::pda::spl_interface(&self.config.pool_mint),
            pool_mint: self.config.pool_mint,
            token_program: self.config.token_program_id,
        });
        instructions::execute_batch(
            ExecuteBatchAccounts {
                payer: self.payer,
                config: self.config_key,
                epoch: self.epoch,
                batch: pda::batch(&self.epoch, self.hash_chain_index).0,
                token_vault: self.token_vault(),
                crank: self.config.crank.signing_pubkey,
                rent_payer: self.rent_payer,
                escrow_tree,
                nullifier_pdas,
                spp,
            },
            pool,
            &ExecuteBatchIxData {
                settlement_proof: proof,
                legs: settlement_legs,
                nullifier_root_index: self.nullifier_root_index,
            },
        )
    }
}

fn single_tree_context(data: &TransactIxData) -> Result<TreeContext> {
    match data.tree_contexts.as_slice() {
        [context] => Ok(*context),
        _ => bail!("settlement legs spend from exactly one tree"),
    }
}

fn proof_data(data: &TransactIxData) -> SppProofData {
    SppProofData {
        proof: data.proof,
        private_tx_hash: data.private_tx_hash,
    }
}

fn compare_ix(leg: &str, expected: &TransactIxData, actual: &TransactIxData) -> Result<()> {
    macro_rules! field {
        ($name:ident) => {
            ensure!(
                expected.$name == actual.$name,
                "{leg} leg: `{}` differs from the program's: expected {:?}, got {:?}",
                stringify!($name),
                expected.$name,
                actual.$name
            );
        };
    }
    field!(expiry_unix_ts);
    field!(tx_viewing_pk);
    field!(salt);
    field!(interface_transfers);
    field!(data_hash);
    field!(ring_data_hash);
    field!(outputs);
    field!(messages);
    field!(private_tx_hash);
    field!(circuit);
    field!(proof);
    field!(inputs);
    field!(tree_contexts);
    // Catches any field added to `TransactIxData` but not listed above.
    ensure!(
        expected == actual,
        "{leg} leg: instruction data differs from the program's"
    );
    Ok(())
}

impl SettlementProofInputs {
    pub fn witness(&self) -> Result<ProofInputMap> {
        let mut map = ProofInputMap::new();
        let c = &self.context;
        let u64_field = |value: u64| right_align(&value.to_be_bytes());
        for (name, value) in [
            ("PublicInputHash", c.hash().map_err(hashing)?),
            ("Context_SpendPrivateTxHash", c.spend_private_tx_hash),
            ("Context_PayoutPrivateTxHash", c.payout_private_tx_hash),
            ("Context_CachedUtxoHash", c.cached_utxo_hash),
            ("Context_UtxoHashChain", c.utxo_hash_chain),
            (
                "Context_IntentCount",
                right_align(&c.intent_count.to_be_bytes()),
            ),
            ("Context_EpochHash", c.epoch_hash),
            ("Context_PoolMintHash", c.pool_mint_hash),
            ("Context_CrankViewingHash", c.crank_viewing_hash),
            ("Context_EscrowOwnerIdentity", c.escrow_owner_identity),
            ("Context_TreeID", right_align(&c.tree_id.to_be_bytes())),
            ("Context_NullifierRoot", self.nullifier_root),
            ("Context_Pool_Lamports", u64_field(self.pool.total_lamports)),
            (
                "Context_Pool_Supply",
                u64_field(self.pool.pool_token_supply),
            ),
            (
                "Context_Pool_DepositNumerator",
                u64_field(self.pool.deposit_fee.numerator),
            ),
            (
                "Context_Pool_DepositDenominator",
                u64_field(self.pool.deposit_fee.denominator),
            ),
            (
                "Context_Pool_WithdrawalNumerator",
                u64_field(self.pool.withdrawal_fee.numerator),
            ),
            (
                "Context_Pool_WithdrawalDenominator",
                u64_field(self.pool.withdrawal_fee.denominator),
            ),
            (
                "Context_Amounts_EscrowSol",
                u64_field(self.amounts.escrow_sol),
            ),
            (
                "Context_Amounts_EscrowPoolTokens",
                u64_field(self.amounts.escrow_pool_tokens),
            ),
            (
                "Context_Amounts_PayoutSol",
                u64_field(self.amounts.payout_sol),
            ),
            (
                "Context_Amounts_PayoutPoolTokens",
                u64_field(self.amounts.payout_pool_tokens),
            ),
            ("WithdrawalTxBlinding", self.withdrawal_tx_blinding),
            ("PayoutTxBlinding", self.payout_tx_blinding),
        ] {
            scalar(&mut map, name, &value);
        }
        bytes(
            &mut map,
            "Encryption_EphemeralSecret",
            &self.ephemeral.to_bytes(),
        );
        bytes(
            &mut map,
            "Encryption_CrankPoint",
            &uncompressed(&self.crank_viewing_pk)?,
        );
        bytes(
            &mut map,
            "Encryption_TxViewingPK",
            &compressed(&self.ephemeral.public_key())?,
        );
        bytes(&mut map, "Encryption_Salt", &self.salt);

        ensure!(
            !self.slots.is_empty() && self.slots.len() <= N,
            "a settlement proof covers 1..={N} entries"
        );
        let first = *self
            .slots
            .first()
            .ok_or_else(|| anyhow!("no settlement slots"))?;
        let absent = SettlementSlot {
            present: false,
            cancelled: false,
            nullifier_inclusion: None,
            ..first
        };
        let padded = self
            .slots
            .iter()
            .copied()
            .chain(std::iter::repeat(absent))
            .take(N);
        for (index, slot) in padded.enumerate() {
            let prefix = format!("Escrow{index}");
            ensure!(
                slot.cancelled || slot.nullifier_inclusion.is_none(),
                "slot {index}: only a cancelled entry has a nullifier inclusion"
            );
            // Queued and absent slots pass zeros; the circuit ignores them.
            let inclusion = slot.nullifier_inclusion.unwrap_or(NullifierInclusion {
                next_value: [0; 32],
                path: [[0; 32]; NULLIFIER_TREE_HEIGHT],
                leaf_index: 0,
            });
            for (name, value) in [
                ("Present", right_align(&[u8::from(slot.present)])),
                ("Cancelled", right_align(&[u8::from(slot.cancelled)])),
                (
                    "CancelledInTree",
                    right_align(&[u8::from(slot.nullifier_inclusion.is_some())]),
                ),
                ("NullifierNextValue", inclusion.next_value),
                ("NullifierPathIndex", u64_field(inclusion.leaf_index)),
                ("PoolAsset", right_align(&[u8::from(slot.pool_asset)])),
                ("Amount", u64_field(slot.amount)),
                ("Blinding", slot.blinding),
                ("NullifierPK", slot.nullifier_pk),
                ("PayoutBlinding", slot.payout_blinding),
            ] {
                scalar(&mut map, &format!("{prefix}_{name}"), &value);
            }
            map.insert(
                format!("{prefix}_NullifierPathElements"),
                inclusion
                    .path
                    .iter()
                    .map(zolana_gnark_ffi_prover::decimal)
                    .collect(),
            );
            bytes(&mut map, &format!("{prefix}_SigningPK"), &slot.signing_pk);
            bytes(
                &mut map,
                &format!("{prefix}_UserPoint"),
                &uncompressed(&slot.viewing_pk)?,
            );
        }
        Ok(map)
    }

    pub fn prove(&self) -> Result<LogicProof> {
        let proof = PROVER
            .prove(LogicCircuit::Settlement, &self.witness()?)?
            .compress()?;
        let (commitment, commitment_pok) = proof
            .commitment
            .ok_or_else(|| anyhow!("missing BSB22 commitment"))?;
        Ok(LogicProof {
            a: proof.proof_a,
            b: proof.proof_b,
            c: proof.proof_c,
            commitment,
            commitment_pok,
        })
    }
}
