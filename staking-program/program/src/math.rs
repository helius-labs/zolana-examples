use spl_stake_pool::state::Fee;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MathError {
    Overflow,
    InvalidFee,
    DepositTooSmall,
    WithdrawalTooSmall,
    InvalidPool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PoolSnapshot {
    pub total_lamports: u64,
    pub pool_token_supply: u64,
    pub deposit_fee: Fee,
    pub withdrawal_fee: Fee,
}

impl PoolSnapshot {
    pub fn deposit_tokens(&self, lamports: u64) -> Result<u64, MathError> {
        if lamports == 0 {
            return Ok(0);
        }
        let gross = self.gross_deposit_tokens(lamports)?;
        let net = gross
            .checked_sub(fee_amount(self.deposit_fee, gross)?)
            .ok_or(MathError::InvalidFee)?;
        if net == 0 {
            return Err(MathError::DepositTooSmall);
        }
        Ok(net)
    }

    pub fn withdrawal_lamports(&self, pool_tokens: u64) -> Result<u64, MathError> {
        if pool_tokens == 0 {
            return Ok(0);
        }
        let burnt = pool_tokens
            .checked_sub(fee_amount(self.withdrawal_fee, pool_tokens)?)
            .ok_or(MathError::InvalidFee)?;
        let lamports = self.gross_withdrawal_lamports(burnt)?;
        if lamports == 0 {
            return Err(MathError::WithdrawalTooSmall);
        }
        Ok(lamports)
    }

    pub fn gross_deposit_tokens(&self, lamports: u64) -> Result<u64, MathError> {
        if self.total_lamports == 0 || self.pool_token_supply == 0 {
            return Ok(lamports);
        }
        mul_div(lamports, self.pool_token_supply, self.total_lamports)
    }

    pub fn gross_withdrawal_lamports(&self, pool_tokens: u64) -> Result<u64, MathError> {
        mul_div(pool_tokens, self.total_lamports, self.pool_token_supply)
    }
}

pub fn mul_div(amount: u64, numerator: u64, denominator: u64) -> Result<u64, MathError> {
    if denominator == 0 {
        return Err(MathError::InvalidPool);
    }
    let value = u128::from(amount)
        .checked_mul(u128::from(numerator))
        .ok_or(MathError::Overflow)?
        .checked_div(u128::from(denominator))
        .ok_or(MathError::InvalidPool)?;
    u64::try_from(value).map_err(|_| MathError::Overflow)
}

fn fee_amount(fee: Fee, amount: u64) -> Result<u64, MathError> {
    if fee.denominator != 0 && fee.numerator > fee.denominator {
        return Err(MathError::InvalidFee);
    }
    let amount = fee.apply(amount).ok_or(MathError::Overflow)?;
    u64::try_from(amount).map_err(|_| MathError::Overflow)
}
