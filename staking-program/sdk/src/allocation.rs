use private_staking_program::{
    math::{mul_div, MathError, PoolSnapshot},
    N,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IntentAmount {
    Deposit(u64),
    Withdrawal(u64),
    Cancelled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PoolOperation {
    None,
    DepositSol { lamports: u64, pool_tokens: u64 },
    WithdrawSol { pool_tokens: u64, lamports: u64 },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SettlementAmounts {
    pub escrow_sol: u64,
    pub escrow_pool_tokens: u64,
    pub payout_sol: u64,
    pub payout_pool_tokens: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Allocation {
    pub utxos: Vec<(IntentAmount, Option<u64>)>,
    pub operation: PoolOperation,
    pub amounts: SettlementAmounts,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AllocationError {
    InvalidIntentCount,
    ZeroAmount,
    Math(MathError),
}

impl From<MathError> for AllocationError {
    fn from(error: MathError) -> Self {
        Self::Math(error)
    }
}

pub fn allocate(
    pool: &PoolSnapshot,
    inputs: &[IntentAmount],
) -> Result<Allocation, AllocationError> {
    if inputs.is_empty() || inputs.len() > N {
        return Err(AllocationError::InvalidIntentCount);
    }
    let mut amounts = SettlementAmounts::default();
    for input in inputs {
        match *input {
            IntentAmount::Deposit(0) | IntentAmount::Withdrawal(0) => {
                return Err(AllocationError::ZeroAmount);
            }
            IntentAmount::Deposit(amount) => {
                amounts.escrow_sol = amounts
                    .escrow_sol
                    .checked_add(amount)
                    .ok_or(MathError::Overflow)?;
            }
            IntentAmount::Withdrawal(amount) => {
                amounts.escrow_pool_tokens = amounts
                    .escrow_pool_tokens
                    .checked_add(amount)
                    .ok_or(MathError::Overflow)?;
            }
            IntentAmount::Cancelled => {}
        }
    }

    let operation = if amounts.escrow_sol == 0 && amounts.escrow_pool_tokens == 0 {
        PoolOperation::None
    } else if amounts.escrow_pool_tokens == 0 {
        amounts.payout_pool_tokens = pool.deposit_tokens(amounts.escrow_sol)?;
        PoolOperation::DepositSol {
            lamports: amounts.escrow_sol,
            pool_tokens: amounts.payout_pool_tokens,
        }
    } else if amounts.escrow_sol == 0 {
        amounts.payout_sol = pool.withdrawal_lamports(amounts.escrow_pool_tokens)?;
        PoolOperation::WithdrawSol {
            pool_tokens: amounts.escrow_pool_tokens,
            lamports: amounts.payout_sol,
        }
    } else {
        let gross_sol = pool.gross_withdrawal_lamports(amounts.escrow_pool_tokens)?;
        if amounts.escrow_sol >= gross_sol {
            amounts.payout_sol = gross_sol;
            let net_sol = amounts
                .escrow_sol
                .checked_sub(gross_sol)
                .ok_or(MathError::Overflow)?;
            let minted = pool.deposit_tokens(net_sol)?;
            amounts.payout_pool_tokens = amounts
                .escrow_pool_tokens
                .checked_add(minted)
                .ok_or(MathError::Overflow)?;
            if net_sol == 0 {
                PoolOperation::None
            } else {
                PoolOperation::DepositSol {
                    lamports: net_sol,
                    pool_tokens: minted,
                }
            }
        } else {
            amounts.payout_pool_tokens = pool.gross_deposit_tokens(amounts.escrow_sol)?;
            let net_tokens = amounts
                .escrow_pool_tokens
                .checked_sub(amounts.payout_pool_tokens)
                .ok_or(MathError::Overflow)?;
            let withdrawn = pool.withdrawal_lamports(net_tokens)?;
            amounts.payout_sol = amounts
                .escrow_sol
                .checked_add(withdrawn)
                .ok_or(MathError::Overflow)?;
            if net_tokens == 0 {
                PoolOperation::None
            } else {
                PoolOperation::WithdrawSol {
                    pool_tokens: net_tokens,
                    lamports: withdrawn,
                }
            }
        }
    };

    let mut deposit_prefix = 0u64;
    let mut deposit_paid = 0u64;
    let mut withdrawal_prefix = 0u64;
    let mut withdrawal_paid = 0u64;
    let mut utxos = Vec::with_capacity(inputs.len());
    for input in inputs {
        let output = match *input {
            IntentAmount::Deposit(amount) => {
                deposit_prefix = deposit_prefix
                    .checked_add(amount)
                    .ok_or(MathError::Overflow)?;
                let paid = mul_div(
                    deposit_prefix,
                    amounts.payout_pool_tokens,
                    amounts.escrow_sol,
                )?;
                let output = paid.checked_sub(deposit_paid).ok_or(MathError::Overflow)?;
                deposit_paid = paid;
                Some(output)
            }
            IntentAmount::Withdrawal(amount) => {
                withdrawal_prefix = withdrawal_prefix
                    .checked_add(amount)
                    .ok_or(MathError::Overflow)?;
                let paid = mul_div(
                    withdrawal_prefix,
                    amounts.payout_sol,
                    amounts.escrow_pool_tokens,
                )?;
                let output = paid
                    .checked_sub(withdrawal_paid)
                    .ok_or(MathError::Overflow)?;
                withdrawal_paid = paid;
                Some(output)
            }
            IntentAmount::Cancelled => None,
        };
        utxos.push((*input, output));
    }
    Ok(Allocation {
        utxos,
        operation,
        amounts,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use spl_stake_pool::state::Fee;

    fn pool() -> PoolSnapshot {
        PoolSnapshot {
            total_lamports: 1_000_000,
            pool_token_supply: 1_000_000,
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
    fn matched_intents_pay_no_pool_operation_fee() {
        let allocation = allocate(
            &pool(),
            &[IntentAmount::Deposit(100), IntentAmount::Withdrawal(100)],
        )
        .unwrap();
        assert_eq!(
            allocation,
            Allocation {
                utxos: vec![
                    (IntentAmount::Deposit(100), Some(100)),
                    (IntentAmount::Withdrawal(100), Some(100))
                ],
                operation: PoolOperation::None,
                amounts: SettlementAmounts {
                    escrow_sol: 100,
                    escrow_pool_tokens: 100,
                    payout_sol: 100,
                    payout_pool_tokens: 100
                },
            }
        );
    }

    #[test]
    fn pool_fees_apply_only_to_net_deposit_and_rounding_leaves_no_dust() {
        let allocation = allocate(
            &pool(),
            &[
                IntentAmount::Deposit(30),
                IntentAmount::Cancelled,
                IntentAmount::Withdrawal(20),
                IntentAmount::Deposit(70),
            ],
        )
        .unwrap();
        assert_eq!(
            allocation,
            Allocation {
                utxos: vec![
                    (IntentAmount::Deposit(30), Some(29)),
                    (IntentAmount::Cancelled, None),
                    (IntentAmount::Withdrawal(20), Some(20)),
                    (IntentAmount::Deposit(70), Some(70))
                ],
                operation: PoolOperation::DepositSol {
                    lamports: 80,
                    pool_tokens: 79
                },
                amounts: SettlementAmounts {
                    escrow_sol: 100,
                    escrow_pool_tokens: 20,
                    payout_sol: 20,
                    payout_pool_tokens: 99
                },
            }
        );
    }

    #[test]
    fn pool_fees_apply_only_to_net_withdrawal() {
        let allocation = allocate(
            &pool(),
            &[
                IntentAmount::Withdrawal(30),
                IntentAmount::Deposit(20),
                IntentAmount::Withdrawal(70),
            ],
        )
        .unwrap();
        assert_eq!(
            allocation,
            Allocation {
                utxos: vec![
                    (IntentAmount::Withdrawal(30), Some(29)),
                    (IntentAmount::Deposit(20), Some(20)),
                    (IntentAmount::Withdrawal(70), Some(70))
                ],
                operation: PoolOperation::WithdrawSol {
                    pool_tokens: 80,
                    lamports: 79
                },
                amounts: SettlementAmounts {
                    escrow_sol: 20,
                    escrow_pool_tokens: 100,
                    payout_sol: 99,
                    payout_pool_tokens: 20
                },
            }
        );
    }

    #[test]
    fn overflowing_aggregate_input_is_rejected() {
        assert_eq!(
            allocate(
                &pool(),
                &[IntentAmount::Deposit(u64::MAX), IntentAmount::Deposit(1)]
            ),
            Err(AllocationError::Math(MathError::Overflow))
        );
    }

    #[test]
    fn all_cancelled_intents_have_no_funds_or_pool_operation() {
        assert_eq!(
            allocate(&pool(), &[IntentAmount::Cancelled]).unwrap(),
            Allocation {
                utxos: vec![(IntentAmount::Cancelled, None)],
                operation: PoolOperation::None,
                amounts: SettlementAmounts::default(),
            }
        );
    }
}
