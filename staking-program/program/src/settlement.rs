use wincode::{SchemaRead, SchemaWrite};

use crate::math::{MathError, PoolSnapshot};

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

/// SPP's content-less address input lets the payout deposit spend no user's
/// funds and retain no private program reserve between transactions.
pub fn payout_address(
    config: &pinocchio::Address,
    batch: &pinocchio::Address,
    tree_id: u16,
) -> Result<[u8; 32], pinocchio::error::ProgramError> {
    use zolana_hasher::{
        primitives::{hash_bytes, right_align, solana_owner_identity},
        Hasher, Poseidon,
    };
    let error = |_| crate::error::StakingError::HashingFailed;
    let seed = hash_bytes(batch.as_array()).map_err(error)?;
    let signer =
        solana_owner_identity(crate::pda::sol_vault(config).0.as_array()).map_err(error)?;
    let nullifier_pk = Poseidon::hashv(&[&[0; 32]]).map_err(error)?;
    let owner = Poseidon::hashv(&[&signer, &nullifier_pk]).map_err(error)?;
    let owner_blinding = Poseidon::hashv(&[&owner, &seed]).map_err(error)?;
    let ring = Poseidon::hashv(&[&[0; 32], &[0; 32]]).map_err(error)?;
    let hash = Poseidon::hashv(&[
        &right_align(&[2]),
        &right_align(&tree_id.to_be_bytes()),
        &[0; 32],
        &[0; 32],
        &[0; 32],
        &ring,
        &owner_blinding,
    ])
    .map_err(error)?;
    Poseidon::hashv(&[&hash, &seed, &[0; 32]])
        .map_err(|_| crate::error::StakingError::HashingFailed.into())
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
}
