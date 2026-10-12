use pinocchio::{error::ProgramError, Address};
use wincode::{SchemaRead, SchemaWrite};
use zolana_hasher::{
    primitives::{hash_bytes, right_align, solana_owner_identity},
    Hasher, Poseidon,
};
use zolana_interface::{
    instruction::instruction_data::transact::{
        CacheAccess, CacheWrite, CircuitId, InputUtxo, InterfaceTransfer, OwnerTag, TransactIxData,
        TransactOutput, TransactProof, TreeContext, MAX_CACHE_WRITES, NO_UTXO_ROOT,
    },
    N_PUBLIC_SLOTS, SOL_ASSET_FIELD, UTXO_DOMAIN,
};
use zolana_program::compression::{PdaOwner, NO_RING_HASH};

use crate::{
    error::StakingError,
    instructions::proof,
    math::{MathError, PoolSnapshot},
    pda,
    state::{BatchEntry, BatchHeader, Config},
    N,
};

/// `1 || user(44) || crank(44)`: the compact payout note sealed to both readers.
pub const PAYOUT_CIPHERTEXT_BYTES: usize = 89;

const WITHDRAWAL_WRITES: [CacheWrite; MAX_CACHE_WRITES] = {
    let mut writes = CacheAccess::NO_WRITES;
    writes[0] = CacheWrite { output: 0, slot: 0 };
    writes
};

/// Spends up to `N` escrows and writes the cached UTXO (output 0) to slot 0.
pub const WITHDRAWAL_CIRCUIT: CircuitId = CircuitId::ConfidentialEddsaCached(
    N as u8,
    2,
    N_PUBLIC_SLOTS as u8,
    CacheAccess {
        read_bitmap: 0,
        write_slots: WITHDRAWAL_WRITES,
    },
);

/// Reads the cached UTXO as input 0 and deposits up to `N` payouts.
pub const PAYOUT_CIRCUIT: CircuitId = CircuitId::ConfidentialEddsaCached(
    1,
    N as u8,
    N_PUBLIC_SLOTS as u8,
    CacheAccess {
        read_bitmap: 1,
        write_slots: CacheAccess::NO_WRITES,
    },
);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, SchemaRead, SchemaWrite)]
pub struct SettlementAmounts {
    pub escrow_sol: u64,
    pub escrow_pool_tokens: u64,
    pub payout_sol: u64,
    pub payout_pool_tokens: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PoolOperation {
    None,
    DepositSol { lamports: u64, pool_tokens: u64 },
    WithdrawSol { pool_tokens: u64, lamports: u64 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, SchemaRead, SchemaWrite)]
pub struct SppProofData {
    pub proof: TransactProof,
    pub private_tx_hash: [u8; 32],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, SchemaRead, SchemaWrite)]
pub struct SettlementLegs {
    pub amounts: SettlementAmounts,
    pub escrow_tree_context: TreeContext,
    pub payout_nullifier_root_index: u16,
    pub withdrawal_output_seed: [u8; 32],
    pub spend: SppProofData,
    pub payout: SppProofData,
}

/// The zero-value SOL UTXO the withdrawal leg caches for the payout leg.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CachedUtxo {
    pub blinding: [u8; 32],
    pub hash: [u8; 32],
    pub nullifier: [u8; 32],
}

/// Mirrors SPP's output commitment for a `sol_vault`-owned, zero-amount SOL
/// UTXO with `data_hash = 0` outside every ring. `DataUtxo::hash` rejects a
/// zero data hash, so the fields are hashed here in its order.
pub fn cached_utxo(
    sol_vault: &Address,
    first_active_nullifier: &[u8; 32],
    output_seed: &[u8; 32],
    tree_id: u16,
) -> Result<CachedUtxo, ProgramError> {
    let error = |_| StakingError::HashingFailed;
    // zolana sdk-libs/program/src/derivation.rs: Poseidon(TXOB, first_nullifier, seed, 0).
    let blinding =
        zolana_program::derive_transact_output_blinding(first_active_nullifier, output_seed, 0)
            .map_err(error)?;
    // compression/owner.rs: Poseidon(solana_owner_identity(pda), Poseidon(0)).
    let owner = PdaOwner::new(sol_vault).map_err(|_| StakingError::HashingFailed)?;
    // compression/utxo.rs DataUtxo::hash (and transaction utxo/output.rs
    // SppProofOutputUtxo::hash): domain, tree id, asset, amount, data hash,
    // ring hash, Poseidon(owner_hash, blinding).
    let owner_blinding = Poseidon::hashv(&[owner.owner_hash(), &blinding]).map_err(error)?;
    let hash = Poseidon::hashv(&[
        &right_align(&UTXO_DOMAIN.to_be_bytes()),
        &right_align(&tree_id.to_be_bytes()),
        &SOL_ASSET_FIELD,
        &[0; 32],
        &[0; 32],
        &NO_RING_HASH,
        &owner_blinding,
    ])
    .map_err(error)?;
    // compression/utxo.rs DataUtxo::key: the zero-secret nullifier.
    let nullifier = Poseidon::hashv(&[&hash, &blinding, &[0; 32]]).map_err(error)?;
    Ok(CachedUtxo {
        blinding,
        hash,
        nullifier,
    })
}

/// Fields of the settlement proof's single public input, in hash order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SettlementProofContext {
    pub spend_private_tx_hash: [u8; 32],
    pub payout_private_tx_hash: [u8; 32],
    pub cached_utxo_hash: [u8; 32],
    pub utxo_hash_chain: [u8; 32],
    pub intent_count: u16,
    pub epoch_hash: [u8; 32],
    pub pool_mint_hash: [u8; 32],
    pub crank_viewing_hash: [u8; 32],
    pub escrow_owner_identity: [u8; 32],
    pub tree_id: u16,
    pub pool_hash: [u8; 32],
    pub batch_hash: [u8; 32],
}

impl SettlementProofContext {
    pub fn hash(&self) -> Result<[u8; 32], ProgramError> {
        Poseidon::hashv(&[
            &self.spend_private_tx_hash,
            &self.payout_private_tx_hash,
            &self.cached_utxo_hash,
            &self.utxo_hash_chain,
            &right_align(&self.intent_count.to_be_bytes()),
            &self.epoch_hash,
            &self.pool_mint_hash,
            &self.crank_viewing_hash,
            &self.escrow_owner_identity,
            &right_align(&self.tree_id.to_be_bytes()),
            &self.pool_hash,
            &self.batch_hash,
        ])
        .map_err(|_| StakingError::HashingFailed.into())
    }
}

pub fn pool_hash(pool: &PoolSnapshot) -> Result<[u8; 32], ProgramError> {
    Poseidon::hashv(&[
        &right_align(&pool.total_lamports.to_be_bytes()),
        &right_align(&pool.pool_token_supply.to_be_bytes()),
        &right_align(&pool.deposit_fee.numerator.to_be_bytes()),
        &right_align(&pool.deposit_fee.denominator.to_be_bytes()),
        &right_align(&pool.withdrawal_fee.numerator.to_be_bytes()),
        &right_align(&pool.withdrawal_fee.denominator.to_be_bytes()),
    ])
    .map_err(|_| StakingError::HashingFailed.into())
}

pub fn batch_hash(
    amounts: &SettlementAmounts,
    entries_hash: &[u8; 32],
) -> Result<[u8; 32], ProgramError> {
    Poseidon::hashv(&[
        &right_align(&amounts.escrow_sol.to_be_bytes()),
        &right_align(&amounts.escrow_pool_tokens.to_be_bytes()),
        &right_align(&amounts.payout_sol.to_be_bytes()),
        &right_align(&amounts.payout_pool_tokens.to_be_bytes()),
        entries_hash,
    ])
    .map_err(|_| StakingError::HashingFailed.into())
}

pub fn entry_leaf(
    nullifier_hash: &[u8; 32],
    cancelled: bool,
    output_hash: Option<&[u8; 32]>,
    ciphertext_context_hash: Option<&[u8; 32]>,
) -> Result<[u8; 32], ProgramError> {
    Poseidon::hashv(&[
        nullifier_hash,
        &right_align(&[u8::from(cancelled)]),
        output_hash.unwrap_or(&[0; 32]),
        ciphertext_context_hash.unwrap_or(&[0; 32]),
    ])
    .map_err(|_| StakingError::HashingFailed.into())
}

/// One step of the entries chain, which starts from zero.
pub fn fold_entry(acc: &[u8; 32], leaf: &[u8; 32]) -> Result<[u8; 32], ProgramError> {
    Poseidon::hashv(&[acc, leaf]).map_err(|_| StakingError::HashingFailed.into())
}

/// Folds every stored entry; `Some` outputs bind their payout ciphertext context.
pub fn entries_hash(
    entries: &[BatchEntry],
    tx_viewing_pk: &[u8; 33],
    salt: &[u8; 16],
) -> Result<[u8; 32], ProgramError> {
    let pk_hash = hash_bytes(tx_viewing_pk).map_err(|_| StakingError::HashingFailed)?;
    entries.iter().try_fold([0; 32], |acc, (input, output)| {
        let leaf = match output {
            Some(output) => {
                let data = output
                    .data
                    .as_deref()
                    .ok_or(StakingError::InvalidCiphertext)?;
                let context = proof::ciphertext_context_hash_with_pk_hash::<
                    PAYOUT_CIPHERTEXT_BYTES,
                >(data, &pk_hash, salt)?;
                entry_leaf(
                    &input.nullifier_hash,
                    false,
                    Some(&output.utxo_hash),
                    Some(&context),
                )?
            }
            None => entry_leaf(&input.nullifier_hash, true, None, None)?,
        };
        fold_entry(&acc, &leaf)
    })
}

#[allow(clippy::too_many_arguments)]
pub fn proof_context(
    config_key: &Address,
    config: &Config,
    epoch: &Address,
    batch: &BatchHeader,
    entries: &[BatchEntry],
    tree_id: u16,
    pool: &PoolSnapshot,
    legs: Option<(&SettlementLegs, &CachedUtxo)>,
) -> Result<SettlementProofContext, ProgramError> {
    let hashing = |_| StakingError::HashingFailed;
    let entries_hash = entries_hash(entries, &batch.tx_viewing_pk, &batch.salt)?;
    let amounts = legs.map(|(legs, _)| legs.amounts).unwrap_or_default();
    let authority = pda::escrow_authority(config_key).0;
    Ok(SettlementProofContext {
        spend_private_tx_hash: legs.map_or([0; 32], |(legs, _)| legs.spend.private_tx_hash),
        payout_private_tx_hash: legs.map_or([0; 32], |(legs, _)| legs.payout.private_tx_hash),
        cached_utxo_hash: legs.map_or([0; 32], |(_, cached)| cached.hash),
        utxo_hash_chain: batch.utxo_hash_chain,
        intent_count: batch.intent_count,
        epoch_hash: hash_bytes(epoch.as_array()).map_err(hashing)?,
        pool_mint_hash: hash_bytes(config.pool_mint.as_array()).map_err(hashing)?,
        // The batch records the config's crank; its ciphertexts are sealed to it.
        crank_viewing_hash: hash_bytes(&batch.crank.viewing_pubkey).map_err(hashing)?,
        escrow_owner_identity: solana_owner_identity(authority.as_array()).map_err(hashing)?,
        tree_id,
        pool_hash: pool_hash(pool)?,
        batch_hash: batch_hash(&amounts, &entries_hash)?,
    })
}

/// Transaction fields shared by both SPP legs.
#[derive(Clone, Copy, Debug)]
pub struct LegContext<'a> {
    pub expiry_unix_ts: u64,
    pub tx_viewing_pk: [u8; 33],
    pub salt: [u8; 16],
    pub spl_interface_bump: u8,
    pub sol_vault: &'a Address,
}

/// SPP rejects zero-amount legs, so each is omitted when its amount is zero.
fn legs(
    sol: (u64, fn(u64) -> InterfaceTransfer),
    spl: (u64, fn(u64, u8) -> InterfaceTransfer),
    bump: u8,
) -> Vec<InterfaceTransfer> {
    let mut transfers = Vec::with_capacity(2);
    if sol.0 != 0 {
        transfers.push(sol.1(sol.0));
    }
    if spl.0 != 0 {
        transfers.push(spl.1(spl.0, bump));
    }
    transfers
}

/// Spends the active escrows into the public vaults and writes the cached UTXO.
pub fn withdrawal_ix_data(
    ctx: &LegContext,
    active_inputs: &[InputUtxo],
    cached: &CachedUtxo,
    amounts: &SettlementAmounts,
    tree_context: TreeContext,
    spend: &SppProofData,
) -> Result<TransactIxData, ProgramError> {
    if active_inputs.is_empty() || active_inputs.len() > N {
        return Err(StakingError::InvalidInstructionData.into());
    }
    Ok(TransactIxData {
        expiry_unix_ts: ctx.expiry_unix_ts,
        tx_viewing_pk: ctx.tx_viewing_pk,
        salt: ctx.salt,
        interface_transfers: legs(
            (amounts.escrow_sol, |amount| {
                InterfaceTransfer::SolWithdrawal { amount }
            }),
            (amounts.escrow_pool_tokens, |amount, spl_interface_bump| {
                InterfaceTransfer::SplWithdrawal {
                    amount,
                    spl_interface_bump,
                }
            }),
            ctx.spl_interface_bump,
        ),
        data_hash: None,
        ring_data_hash: None,
        outputs: vec![TransactOutput {
            utxo_hash: cached.hash,
            owner_tag: OwnerTag::Inline(ctx.sol_vault.to_bytes()),
            data: None,
        }],
        messages: Vec::new(),
        private_tx_hash: spend.private_tx_hash,
        circuit: WITHDRAWAL_CIRCUIT,
        proof: spend.proof,
        inputs: active_inputs.to_vec(),
        tree_contexts: vec![tree_context],
    })
}

/// Spends the cached UTXO and deposits the stored payouts from the vaults.
pub fn payout_ix_data(
    ctx: &LegContext,
    outputs: Vec<TransactOutput>,
    cached: &CachedUtxo,
    amounts: &SettlementAmounts,
    payout_nullifier_root_index: u16,
    payout: &SppProofData,
) -> Result<TransactIxData, ProgramError> {
    if outputs.is_empty() || outputs.len() > N {
        return Err(StakingError::InvalidInstructionData.into());
    }
    Ok(TransactIxData {
        expiry_unix_ts: ctx.expiry_unix_ts,
        tx_viewing_pk: ctx.tx_viewing_pk,
        salt: ctx.salt,
        interface_transfers: legs(
            (amounts.payout_sol, |amount| InterfaceTransfer::SolDeposit {
                amount,
            }),
            (amounts.payout_pool_tokens, |amount, spl_interface_bump| {
                InterfaceTransfer::SplDeposit {
                    amount,
                    spl_interface_bump,
                }
            }),
            ctx.spl_interface_bump,
        ),
        data_hash: None,
        ring_data_hash: None,
        outputs,
        messages: Vec::new(),
        private_tx_hash: payout.private_tx_hash,
        circuit: PAYOUT_CIRCUIT,
        proof: payout.proof,
        inputs: vec![InputUtxo {
            nullifier_hash: cached.nullifier,
            tree_index: 0,
        }],
        tree_contexts: vec![TreeContext {
            utxo_tree_root_index: NO_UTXO_ROOT,
            nullifier_tree_root_index: payout_nullifier_root_index,
        }],
    })
}

impl SettlementAmounts {
    /// Only a net deposit, a net withdrawal, or exact matching is permitted.
    /// The pool quote authenticates the other asset's corresponding deficit.
    pub fn pool_operation(&self, pool: &PoolSnapshot) -> Result<PoolOperation, MathError> {
        match (
            self.escrow_sol.cmp(&self.payout_sol),
            self.escrow_pool_tokens.cmp(&self.payout_pool_tokens),
        ) {
            (core::cmp::Ordering::Equal, core::cmp::Ordering::Equal) => Ok(PoolOperation::None),
            (core::cmp::Ordering::Greater, core::cmp::Ordering::Less) => {
                let lamports = self
                    .escrow_sol
                    .checked_sub(self.payout_sol)
                    .ok_or(MathError::Overflow)?;
                let tokens = self
                    .payout_pool_tokens
                    .checked_sub(self.escrow_pool_tokens)
                    .ok_or(MathError::Overflow)?;
                if pool.deposit_tokens(lamports)? != tokens {
                    return Err(MathError::InvalidPool);
                }
                Ok(PoolOperation::DepositSol {
                    lamports,
                    pool_tokens: tokens,
                })
            }
            (core::cmp::Ordering::Less, core::cmp::Ordering::Greater) => {
                let tokens = self
                    .escrow_pool_tokens
                    .checked_sub(self.payout_pool_tokens)
                    .ok_or(MathError::Overflow)?;
                let lamports = self
                    .payout_sol
                    .checked_sub(self.escrow_sol)
                    .ok_or(MathError::Overflow)?;
                if pool.withdrawal_lamports(tokens)? != lamports {
                    return Err(MathError::InvalidPool);
                }
                Ok(PoolOperation::WithdrawSol {
                    pool_tokens: tokens,
                    lamports,
                })
            }
            _ => Err(MathError::InvalidPool),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spl_stake_pool::state::Fee;

    fn pool() -> PoolSnapshot {
        PoolSnapshot {
            total_lamports: 1_000,
            pool_token_supply: 500,
            deposit_fee: Fee {
                numerator: 1,
                denominator: 100,
            },
            withdrawal_fee: Fee {
                numerator: 1,
                denominator: 100,
            },
        }
    }

    #[test]
    fn derived_operation_balances_both_assets_at_the_actual_fee_quote() {
        let deposit = SettlementAmounts {
            escrow_sol: 200,
            escrow_pool_tokens: 20,
            payout_sol: 40,
            payout_pool_tokens: 99,
        };
        assert_eq!(
            deposit.pool_operation(&pool()),
            Ok(PoolOperation::DepositSol {
                lamports: 160,
                pool_tokens: 79
            })
        );
        let withdrawal = SettlementAmounts {
            escrow_sol: 40,
            escrow_pool_tokens: 100,
            payout_sol: 198,
            payout_pool_tokens: 20,
        };
        assert_eq!(
            withdrawal.pool_operation(&pool()),
            Ok(PoolOperation::WithdrawSol {
                pool_tokens: 80,
                lamports: 158
            })
        );
        assert_eq!(
            SettlementAmounts {
                escrow_sol: 40,
                payout_sol: 40,
                escrow_pool_tokens: 20,
                payout_pool_tokens: 20
            }
            .pool_operation(&pool()),
            Ok(PoolOperation::None)
        );
    }

    #[test]
    fn unbalanced_amounts_cannot_consume_the_existing_vault_balances() {
        let wrong_quote = SettlementAmounts {
            escrow_sol: 200,
            escrow_pool_tokens: 20,
            payout_sol: 40,
            payout_pool_tokens: 100,
        };
        assert_eq!(
            wrong_quote.pool_operation(&pool()),
            Err(MathError::InvalidPool)
        );
        let both_surplus = SettlementAmounts {
            escrow_sol: 200,
            escrow_pool_tokens: 20,
            payout_sol: 0,
            payout_pool_tokens: 0,
        };
        assert_eq!(
            both_surplus.pool_operation(&pool()),
            Err(MathError::InvalidPool)
        );
        let both_deficit = SettlementAmounts {
            escrow_sol: 0,
            escrow_pool_tokens: 0,
            payout_sol: 200,
            payout_pool_tokens: 20,
        };
        assert_eq!(
            both_deficit.pool_operation(&pool()),
            Err(MathError::InvalidPool)
        );
    }

    fn address(byte: u8) -> Address {
        Address::new_from_array([byte; 32])
    }

    fn cached() -> (Address, CachedUtxo) {
        let config = pda::config(&address(10)).0;
        let vault = pda::sol_vault(&config).0;
        let seed = [
            8, 248, 157, 48, 65, 140, 168, 177, 34, 112, 56, 121, 53, 57, 169, 178, 7, 233, 232,
            246, 65, 84, 237, 22, 32, 231, 130, 124, 204, 146, 26, 106,
        ];
        (vault, cached_utxo(&vault, &[3; 32], &seed, 7).unwrap())
    }

    fn ciphertext(byte: u8) -> Vec<u8> {
        core::iter::once(1)
            .chain(core::iter::repeat_n(byte, PAYOUT_CIPHERTEXT_BYTES - 1))
            .collect()
    }

    fn entries() -> Vec<BatchEntry> {
        vec![
            (
                InputUtxo {
                    nullifier_hash: [1; 32],
                    tree_index: 0,
                },
                None,
            ),
            (
                InputUtxo {
                    nullifier_hash: [2; 32],
                    tree_index: 0,
                },
                Some(TransactOutput {
                    utxo_hash: [4; 32],
                    owner_tag: OwnerTag::Inline([5; 32]),
                    data: Some(ciphertext(6)),
                }),
            ),
        ]
    }

    fn spp_proof(byte: u8) -> SppProofData {
        SppProofData {
            proof: TransactProof {
                a: [byte; 32],
                b: [byte; 128],
                c: [byte; 32],
            },
            private_tx_hash: [byte; 32],
        }
    }

    /// Expected bytes from `sdk/tests/cached_utxo.rs`, which checks the same
    /// inputs against `SppProofOutputUtxo::hash` and `NullifierKey::nullifier`.
    #[test]
    fn cached_utxo_matches_the_spp_client_vector() {
        let expected = CachedUtxo {
            blinding: [
                14, 138, 249, 155, 231, 116, 255, 85, 128, 82, 86, 223, 234, 88, 137, 82, 118, 187,
                26, 124, 199, 8, 15, 64, 78, 22, 250, 96, 140, 142, 200, 205,
            ],
            hash: [
                0, 253, 175, 38, 229, 45, 152, 216, 56, 241, 190, 103, 97, 3, 26, 159, 100, 25, 65,
                67, 46, 249, 94, 114, 64, 244, 63, 190, 178, 53, 74, 60,
            ],
            nullifier: [
                42, 251, 253, 63, 110, 16, 164, 77, 59, 86, 121, 117, 84, 186, 92, 253, 83, 6, 171,
                204, 88, 79, 150, 6, 174, 211, 193, 179, 52, 62, 149, 55,
            ],
        };
        assert_eq!(cached().1, expected);
    }

    #[test]
    fn context_hash_is_deterministic_and_binds_every_field() {
        let base = SettlementProofContext {
            spend_private_tx_hash: [1; 32],
            payout_private_tx_hash: [2; 32],
            cached_utxo_hash: [3; 32],
            utxo_hash_chain: [4; 32],
            intent_count: 5,
            epoch_hash: [6; 32],
            pool_mint_hash: [7; 32],
            crank_viewing_hash: [8; 32],
            escrow_owner_identity: [9; 32],
            tree_id: 10,
            pool_hash: [11; 32],
            batch_hash: [12; 32],
        };
        let mutations: [fn(&mut SettlementProofContext); 12] = [
            |c| c.spend_private_tx_hash = [0; 32],
            |c| c.payout_private_tx_hash = [0; 32],
            |c| c.cached_utxo_hash = [0; 32],
            |c| c.utxo_hash_chain = [0; 32],
            |c| c.intent_count = 0,
            |c| c.epoch_hash = [0; 32],
            |c| c.pool_mint_hash = [0; 32],
            |c| c.crank_viewing_hash = [0; 32],
            |c| c.escrow_owner_identity = [0; 32],
            |c| c.tree_id = 0,
            |c| c.pool_hash = [0; 32],
            |c| c.batch_hash = [0; 32],
        ];
        let mut hashes: Vec<_> = mutations
            .iter()
            .map(|mutate| {
                let mut context = base;
                mutate(&mut context);
                context.hash().unwrap()
            })
            .collect();
        hashes.push(base.hash().unwrap());
        assert_eq!(base.hash().unwrap(), base.hash().unwrap());
        hashes.sort();
        hashes.dedup();
        assert_eq!(hashes.len(), 13);
    }

    #[test]
    fn entries_fold_in_order_and_bind_payout_ciphertexts() {
        let entries = entries();
        let (pk, salt) = ([2; 33], [7; 16]);
        let context =
            proof::ciphertext_context_hash::<PAYOUT_CIPHERTEXT_BYTES>(&ciphertext(6), &pk, &salt)
                .unwrap();
        let cancelled = entry_leaf(&[1; 32], true, None, None).unwrap();
        let active = entry_leaf(&[2; 32], false, Some(&[4; 32]), Some(&context)).unwrap();
        let expected = fold_entry(&fold_entry(&[0; 32], &cancelled).unwrap(), &active).unwrap();
        let reversed: Vec<_> = entries.iter().rev().cloned().collect();
        let mut tampered = entries.clone();
        if let Some((_, Some(output))) = tampered.get_mut(1) {
            output.data = Some(ciphertext(9));
        }
        assert_eq!(
            (
                entries_hash(&entries, &pk, &salt).unwrap() == expected,
                entries_hash(&reversed, &pk, &salt).unwrap() == expected,
                entries_hash(&tampered, &pk, &salt).unwrap() == expected,
                entries_hash(&[], &pk, &salt).unwrap(),
            ),
            (true, false, false, [0; 32])
        );
    }

    #[test]
    fn proof_context_zeroes_absent_legs() {
        let config = Config {
            discriminator: crate::state::CONFIG_DISCRIMINATOR,
            authority: address(1),
            stake_pool_program_id: address(2),
            stake_pool: address(3),
            pool_mint: address(4),
            token_program_id: address(5),
            escrow_tree: address(6),
            crank: crate::state::ShieldedAddress {
                signing_pubkey: address(7),
                nullifier_pubkey: [8; 32],
                viewing_pubkey: [2; 33],
            },
            intent_fee_lamports: 10,
            settlement_window_seconds: 600,
            min_deposit_lamports: 1,
            min_withdrawal_pool_tokens: 1,
        };
        let batch = BatchHeader {
            discriminator: crate::state::BATCH_DISCRIMINATOR,
            epoch: address(9),
            hash_chain_index: 0,
            utxo_hash_chain: [3; 32],
            intent_count: 2,
            crank: config.crank,
            rent_payer: address(4),
            tx_viewing_pk: [2; 33],
            salt: [5; 16],
            filled_count: 2,
        };
        let (_, cached) = cached();
        let legs = SettlementLegs {
            amounts: SettlementAmounts {
                escrow_sol: 1,
                escrow_pool_tokens: 2,
                payout_sol: 3,
                payout_pool_tokens: 4,
            },
            escrow_tree_context: TreeContext {
                utxo_tree_root_index: 1,
                nullifier_tree_root_index: 2,
            },
            payout_nullifier_root_index: 3,
            withdrawal_output_seed: [4; 32],
            spend: spp_proof(5),
            payout: spp_proof(6),
        };
        let config_key = address(11);
        let context = |legs| {
            proof_context(
                &config_key,
                &config,
                &address(9),
                &batch,
                &entries(),
                7,
                &pool(),
                legs,
            )
            .unwrap()
        };
        let none = context(None);
        let some = context(Some((&legs, &cached)));
        let entries_hash = entries_hash(&entries(), &batch.tx_viewing_pk, &batch.salt).unwrap();
        assert_eq!(
            (
                none.spend_private_tx_hash,
                none.payout_private_tx_hash,
                none.cached_utxo_hash,
                none.batch_hash,
                some.spend_private_tx_hash,
                some.payout_private_tx_hash,
                some.cached_utxo_hash,
                some.batch_hash,
                some.pool_hash,
                some.intent_count,
                some.tree_id,
            ),
            (
                [0; 32],
                [0; 32],
                [0; 32],
                batch_hash(&SettlementAmounts::default(), &entries_hash).unwrap(),
                [5; 32],
                [6; 32],
                cached.hash,
                batch_hash(&legs.amounts, &entries_hash).unwrap(),
                pool_hash(&pool()).unwrap(),
                2,
                7,
            )
        );
    }

    #[test]
    fn leg_builders_omit_zero_transfers_and_serialize() {
        let (vault, cached) = cached();
        let ctx = LegContext {
            expiry_unix_ts: 1_000,
            tx_viewing_pk: [2; 33],
            salt: [5; 16],
            spl_interface_bump: 254,
            sol_vault: &vault,
        };
        let amounts = SettlementAmounts {
            escrow_sol: 0,
            escrow_pool_tokens: 20,
            payout_sol: 30,
            payout_pool_tokens: 0,
        };
        let inputs = [InputUtxo {
            nullifier_hash: [1; 32],
            tree_index: 0,
        }];
        let tree = TreeContext {
            utxo_tree_root_index: 1,
            nullifier_tree_root_index: 2,
        };
        let withdrawal =
            withdrawal_ix_data(&ctx, &inputs, &cached, &amounts, tree, &spp_proof(5)).unwrap();
        let outputs = entries()
            .into_iter()
            .filter_map(|(_, output)| output)
            .collect();
        let payout = payout_ix_data(&ctx, outputs, &cached, &amounts, 3, &spp_proof(6)).unwrap();
        let both = withdrawal_ix_data(
            &ctx,
            &inputs,
            &cached,
            &SettlementAmounts {
                escrow_sol: 10,
                ..amounts
            },
            tree,
            &spp_proof(5),
        )
        .unwrap();
        assert_eq!(
            (
                withdrawal.interface_transfers.clone(),
                payout.interface_transfers.clone(),
                both.interface_transfers.clone(),
                withdrawal.outputs.clone(),
                payout.inputs.clone(),
                payout.tree_contexts.clone(),
            ),
            (
                vec![InterfaceTransfer::SplWithdrawal {
                    amount: 20,
                    spl_interface_bump: 254
                }],
                vec![InterfaceTransfer::SolDeposit { amount: 30 }],
                vec![
                    InterfaceTransfer::SolWithdrawal { amount: 10 },
                    InterfaceTransfer::SplWithdrawal {
                        amount: 20,
                        spl_interface_bump: 254
                    }
                ],
                vec![TransactOutput {
                    utxo_hash: cached.hash,
                    owner_tag: OwnerTag::Inline(vault.to_bytes()),
                    data: None,
                }],
                vec![InputUtxo {
                    nullifier_hash: cached.nullifier,
                    tree_index: 0
                }],
                vec![TreeContext {
                    utxo_tree_root_index: NO_UTXO_ROOT,
                    nullifier_tree_root_index: 3
                }],
            )
        );
        for data in [&withdrawal, &payout, &both] {
            let bytes = data.serialize().unwrap();
            assert_eq!(&TransactIxData::deserialize(&bytes).unwrap(), data);
        }
        assert!(WITHDRAWAL_CIRCUIT.is_supported() && PAYOUT_CIRCUIT.is_supported());
        assert!(WITHDRAWAL_CIRCUIT
            .cache_access()
            .is_some_and(|access| access.valid(N, 2) && access.read_bitmap == 0));
        assert!(PAYOUT_CIRCUIT
            .cache_access()
            .is_some_and(|access| access.valid(1, N) && !access.writes_cache()));
        assert_eq!(
            withdrawal_ix_data(&ctx, &[], &cached, &amounts, tree, &spp_proof(5)),
            Err(StakingError::InvalidInstructionData.into())
        );
    }
}
