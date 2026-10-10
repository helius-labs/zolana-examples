//! A kVault and its token as one tradable pair, and the vault preview: the
//! port of the kVault program's deposit and withdraw pricing that both the
//! user (to check a quote) and the market maker (to quote and rebalance)
//! run. The preview fails with the same condition, named by a
//! [`VaultError`], wherever the program would fail on the same state.

use std::iter;

use anyhow::Result;
use solana_address::Address;
use thiserror::Error;

use crate::{
    kvault::{self, MAX_RESERVES},
    price::{self, Rounding},
    swap::FULL_BPS,
};

/// A kVault and its token: the user swaps `token_mint` for `shares_mint`
/// and back. The three PDAs are derived once by [`Pair::new`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pair {
    /// The kVault `VaultState` account.
    pub vault: Address,
    /// The vault's underlying token, the collateral side of the pair.
    pub token_mint: Address,
    /// The vault's base authority PDA ([`kvault::base_vault_authority`]).
    pub authority: Address,
    /// The vault's token account PDA ([`kvault::token_vault`]).
    pub token_vault: Address,
    /// The vault's share mint PDA ([`kvault::shares_mint`]).
    pub shares_mint: Address,
}

impl Pair {
    /// The pair of `vault` and its `token_mint`. Does not read the chain:
    /// the caller is responsible for `token_mint` being the vault's mint.
    pub fn new(vault: Address, token_mint: Address) -> Self {
        Self {
            vault,
            token_mint,
            authority: kvault::base_vault_authority(&vault),
            token_vault: kvault::token_vault(&vault),
            shares_mint: kvault::shares_mint(&vault),
        }
    }
}

/// Why a vault preview fails. Variants named after a kVault program error
/// fail the same way on chain (`programs/kvault/src/lib.rs`).
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum VaultError {
    /// `AUMBelowPendingFees` (`state.rs:261`).
    #[error(
        "vault holdings {holdings_sf} are below its pending fees {pending_fees_sf} (Fraction bits)"
    )]
    AumBelowPendingFees {
        holdings_sf: u128,
        pending_fees_sf: u128,
    },
    /// `VaultAUMZero` (`vault_operations.rs:1079`, `:164`).
    #[error("vault has shares issued but no assets under management")]
    AumZero,
    /// `VaultDepositCapReached` (`vault_operations.rs:88`, `:112`).
    #[error("vault deposit cap {cap} reached at AUM {aum}")]
    DepositCapReached { cap: u64, aum: u64 },
    /// `DepositAmountBelowMinimum` (`vault_operations.rs:114`).
    #[error("deposit of {amount} is below the vault minimum {minimum}")]
    BelowMinimumDeposit { amount: u64, minimum: u64 },
    /// The program subtracts the crank funds from `max_amount` unchecked
    /// (`vault_operations.rs:68`).
    #[error("deposit of {amount} does not cover the crank funds {crank_funds}")]
    BelowCrankFunds { amount: u64, crank_funds: u64 },
    /// `DepositAmountsZeroShares` (`vault_operations.rs:118`).
    #[error("a deposit of {amount} mints no shares")]
    DepositMintsNoShares { amount: u64 },
    /// `CannotWithdrawZeroShares` (`vault_operations.rs:190`).
    #[error("withdrawing zero shares")]
    WithdrawZeroShares,
    /// More shares withdrawn than the vault has issued; the program fails
    /// the share burn.
    #[error("withdrawing {shares} shares of {issued} issued")]
    SharesExceedIssued { shares: u64, issued: u64 },
    /// `CannotWithdrawZeroLamports` (`vault_operations.rs:209`).
    #[error("a withdrawal of {shares} shares pays no tokens")]
    WithdrawPaysNothing { shares: u64 },
    /// `WithdrawAmountLessThanWithdrawalPenalty` (`vault_operations.rs:216`).
    #[error("withdrawal penalty {penalty} is not below the withdrawn amount {tokens}")]
    PenaltyExceedsWithdrawal { tokens: u64, penalty: u64 },
    /// `WithdrawAmountBelowMinimum` (`vault_operations.rs:320`).
    #[error("withdrawal of {tokens} is not above the vault minimum {minimum}")]
    BelowMinimumWithdraw { tokens: u64, minimum: u64 },
    /// `WithdrawResultsInZeroShares` (`vault_operations.rs:304`).
    #[error("a withdrawal of {shares} shares burns none")]
    WithdrawBurnsNoShares { shares: u64 },
    /// One withdraw instruction draws from `token_available` and at most one
    /// reserve (`vault_operations.rs:223-230`); the program would pay less.
    #[error("withdrawal of {tokens} exceeds the {available} one instruction can pay out")]
    WithdrawExceedsLiquidity { tokens: u64, available: u64 },
    /// An intermediate does not fit its type, where the program panics or
    /// returns `MathOverflow`; `context` names the computation.
    #[error("vault math overflow in {context}")]
    Overflow { context: &'static str },
    /// The caller passed a reserve account list whose length differs from
    /// the vault's allocations.
    #[error("{got} reserve accounts given for {expected} allocations")]
    ReserveCount { expected: usize, got: usize },
    /// `ReserveAccountAndKeyMismatch` (`vault_operations.rs:1118`).
    #[error("allocation {slot} is reserve {expected}, got {got}")]
    ReserveMismatch {
        slot: usize,
        expected: Address,
        got: Address,
    },
}

/// `VaultError::Overflow` naming the computation that overflowed.
pub(crate) fn overflow(context: &'static str) -> VaultError {
    VaultError::Overflow { context }
}

/// One allocated reserve of a vault, as of the reserve's last refresh.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReserveSnapshot {
    /// The klend `Reserve` account.
    pub reserve: Address,
    /// The reserve's `lending_market`; a vault deposit or withdraw passes it
    /// read-only after all reserves (kvault-interface `helpers/common.rs:19`).
    pub lending_market: Address,
    /// The reserve's `liquidity.supply_vault`, the token account a vault
    /// `withdraw` against this reserve redeems from.
    pub liquidity_supply_vault: Address,
    /// The reserve's `collateral.mint_pubkey`, the cToken mint a vault
    /// `withdraw` against this reserve burns.
    pub collateral_mint: Address,
    /// cTokens of this reserve the vault holds (the allocation slot's
    /// `ctoken_allocation`).
    pub ctoken_allocation: u64,
    /// `ctoken_allocation` at the reserve's exchange rate, as `Fraction`
    /// bits (kvault `amounts_invested`, `vault_operations.rs:1124`).
    pub invested_liquidity_sf: u128,
}

/// The allocated reserves of a vault in allocation slot order, the order the
/// program expects them in its remaining accounts. At most the program's
/// `MAX_RESERVES`.
///
/// Heap-backed: a fixed `MAX_RESERVES` array would make every `VaultState`
/// about 4 KB, and the market maker's debug-build futures hold several of
/// them on the stack.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Reserves {
    snapshots: Vec<ReserveSnapshot>,
}

impl Reserves {
    /// No reserve; a vault without allocations.
    pub const EMPTY: Self = Self {
        snapshots: Vec::new(),
    };

    /// The snapshots in allocation slot order.
    pub fn as_slice(&self) -> &[ReserveSnapshot] {
        &self.snapshots
    }

    fn as_mut_slice(&mut self) -> &mut [ReserveSnapshot] {
        &mut self.snapshots
    }

    pub(crate) fn push(&mut self, snapshot: ReserveSnapshot) -> Result<(), VaultError> {
        if self.snapshots.len() >= MAX_RESERVES {
            return Err(overflow("reserve snapshots"));
        }
        self.snapshots.push(snapshot);
        Ok(())
    }

    /// kvault `Invested::total` (`vault_operations.rs:1136`): the sum of the
    /// reserves' liquidity, kept as `Fraction` bits.
    fn invested_total_sf(&self) -> Result<u128, VaultError> {
        self.as_slice().iter().try_fold(0u128, |total, snapshot| {
            total
                .checked_add(snapshot.invested_liquidity_sf)
                .ok_or_else(|| overflow("invested total"))
        })
    }
}

/// A kVault priced the way the kVault program prices its deposits and
/// withdrawals (release 2.2.1, `programs/kvault/src`).
///
/// What the price includes:
/// - `token_available`, the vault's uninvested tokens;
/// - the liquidity value of every allocated reserve's cTokens at that
///   reserve's exchange rate as stored in the reserve account, that is as of
///   the reserve's last refresh (`reserves`);
/// - the vault's `pending_fees_sf`, subtracted from the AUM;
/// - the withdrawal penalty, the larger of the vault's and the global
///   config's bps and lamport settings;
/// - the minimum deposit and withdraw amounts, the deposit cap and the crank
///   funds charged on top of a deposit.
///
/// What it does not simulate:
/// - interest the reserves accrue between their last refresh and the
///   transaction, which the program's reserve refresh adds before pricing;
/// - management and performance fees the program charges at execution
///   (`charge_fees`, `vault_operations.rs:79`, `:157`) before it computes the AUM.
///
/// So the price drifts from the executed one by the interest and fees of the
/// time since the last refresh. That is why the market maker shields the
/// token-account delta it observes after a vault deposit or withdrawal
/// rather than the previewed amount, and why its `fee_bps` has to cover the
/// drift between the quoted and the executed price.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VaultState {
    /// The vault's underlying token mint.
    pub token_mint: Address,
    /// The token program of `token_mint` (SPL Token or Token-2022).
    pub token_program: Address,
    /// Uninvested tokens in the vault's token account.
    pub token_available: u64,
    /// The share mint's supply as the vault records it.
    pub shares_issued: u64,
    /// Accrued management and performance fees not yet withdrawn by the
    /// vault admin, as `Fraction` bits; they are not part of the AUM.
    pub pending_fees_sf: u128,
    /// The allocated reserves, in allocation slot order.
    pub reserves: Reserves,
    /// Smallest token amount a deposit may take (checked after the crank
    /// funds come off).
    pub min_deposit_amount: u64,
    /// A withdrawal must pay strictly more tokens than this.
    pub min_withdraw_amount: u64,
    /// 0 means uncapped.
    pub deposit_cap: u64,
    /// Tokens a deposit pays into the crank fund on top of the deposited
    /// amount: `crank_fund_fee_per_reserve` times the reserves with an
    /// allocation (`vault_operations.rs:66`).
    pub crank_funds: u64,
    /// The vault's flat withdrawal penalty, in token atoms despite the name.
    pub withdrawal_penalty_lamports: u64,
    /// The vault's proportional withdrawal penalty.
    pub withdrawal_penalty_bps: u64,
    /// The `GlobalConfig` flat penalty; the larger of it and the vault's
    /// applies.
    pub global_withdrawal_penalty_lamports: u64,
    /// The `GlobalConfig` proportional penalty; the larger of it and the
    /// vault's applies.
    pub global_withdrawal_penalty_bps: u64,
}

/// The previewed result of [`VaultState::deposit`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DepositOutcome {
    /// Tokens taken from the depositor: the deposited amount plus
    /// `crank_funds` (`handler_deposit.rs:70`).
    pub tokens: u64,
    /// Shares minted to the depositor.
    pub shares: u64,
    /// The vault after the deposit, for previewing a following operation.
    pub after: VaultState,
}

/// The previewed result of [`VaultState::withdraw`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WithdrawOutcome {
    /// Tokens paid to the withdrawer, net of `penalty`.
    pub tokens: u64,
    /// The withdrawal penalty, which stays in the vault.
    pub penalty: u64,
    /// Shares burned.
    pub shares: u64,
    /// The vault after the withdrawal, with the cToken side only partly
    /// updated (see [`VaultState::withdraw`]).
    pub after: VaultState,
}

/// Where a vault withdrawal draws its tokens from
/// ([`VaultState::withdraw_source`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WithdrawSource {
    /// The vault's uninvested `token_available` covers the withdrawal.
    Available,
    /// `token_available` plus a disinvestment from this reserve.
    Reserve(ReserveSnapshot),
}

impl VaultState {
    /// The accounts besides the vault that price it, in the order
    /// [`VaultState::from_accounts`] takes them: the kVault `GlobalConfig`,
    /// then the allocated reserves in allocation slot order.
    pub fn pricing_accounts(vault_data: &[u8]) -> Result<Vec<Address>> {
        let view = kvault::vault_state(vault_data)?;
        Ok(iter::once(kvault::global_config_address())
            .chain(view.allocations.iter().map(|allocation| allocation.reserve))
            .collect())
    }

    /// Prices a vault from its account data, its `GlobalConfig` data and
    /// its allocated reserves' `(address, data)` in allocation slot order,
    /// the order [`VaultState::pricing_accounts`] returns.
    ///
    /// Checks, in order: the vault and global config decode
    /// (`KvaultError::KvaultAccount`); the number of reserves equals the
    /// vault's allocations (`VaultError::ReserveCount`); then per slot, the
    /// reserve is the one in that slot (`VaultError::ReserveMismatch`) and
    /// decodes (`KvaultError::ReserveAccount`). Each slot's cTokens are
    /// valued at the reserve's stored exchange rate, which is why the
    /// address check matters: a wrong reserve would misprice the vault.
    pub fn from_accounts(
        vault_data: &[u8],
        global_config_data: &[u8],
        reserves: &[(Address, &[u8])],
    ) -> Result<Self> {
        let view = kvault::vault_state(vault_data)?;
        let global = kvault::global_config(global_config_data)?;
        let allocations = &view.allocations;
        if allocations.len() != reserves.len() {
            return Err(VaultError::ReserveCount {
                expected: allocations.len(),
                got: reserves.len(),
            }
            .into());
        }
        let mut snapshots = Reserves::EMPTY;
        for (slot, (allocation, (address, data))) in allocations.iter().zip(reserves).enumerate() {
            if allocation.reserve != *address {
                return Err(VaultError::ReserveMismatch {
                    slot,
                    expected: allocation.reserve,
                    got: *address,
                }
                .into());
            }
            let reserve = kvault::reserve(data)?;
            snapshots.push(ReserveSnapshot {
                reserve: *address,
                lending_market: reserve.lending_market,
                liquidity_supply_vault: reserve.liquidity_supply_vault,
                collateral_mint: reserve.collateral_mint,
                ctoken_allocation: allocation.ctoken_allocation,
                invested_liquidity_sf: price::collateral_to_liquidity_sf(
                    &reserve,
                    allocation.ctoken_allocation,
                )?,
            })?;
        }
        Ok(Self {
            token_mint: view.token_mint,
            token_program: view.token_program,
            token_available: view.token_available,
            shares_issued: view.shares_issued,
            pending_fees_sf: view.pending_fees_sf,
            reserves: snapshots,
            min_deposit_amount: view.min_deposit_amount,
            min_withdraw_amount: view.min_withdraw_amount,
            deposit_cap: view.deposit_cap,
            crank_funds: view
                .reserves_with_allocation
                .checked_mul(view.crank_fund_fee_per_reserve)
                .ok_or_else(|| overflow("crank funds"))?,
            withdrawal_penalty_lamports: view.withdrawal_penalty_lamports,
            withdrawal_penalty_bps: view.withdrawal_penalty_bps,
            global_withdrawal_penalty_lamports: global.withdrawal_penalty_lamports,
            global_withdrawal_penalty_bps: global.withdrawal_penalty_bps,
        })
    }

    /// Assets under management as `Fraction` bits: `token_available` plus
    /// the invested liquidity minus the pending fees (`compute_aum`,
    /// `state.rs:256`, exact). Errors with `AumBelowPendingFees` like the
    /// program.
    pub fn aum_sf(&self) -> Result<u128, VaultError> {
        let holdings_sf = price::sf(self.token_available)
            .checked_add(self.reserves.invested_total_sf()?)
            .ok_or_else(|| overflow("vault holdings"))?;
        holdings_sf
            .checked_sub(self.pending_fees_sf)
            .ok_or(VaultError::AumBelowPendingFees {
                holdings_sf,
                pending_fees_sf: self.pending_fees_sf,
            })
    }

    /// The deposit of up to `amount` tokens (the instruction's `max_amount`),
    /// following kvault `deposit` (`vault_operations.rs:51-135`):
    ///
    /// 1. The crank funds come off `amount` (`:68`); error `BelowCrankFunds`.
    /// 2. Room under the cap: `deposit_cap` (0 = uncapped) minus `ceil(aum)`
    ///    (`get_max_depositable_in_vault`, `:962`); error `DepositCapReached`
    ///    when there is none. A larger deposit is cut to the room (`:90`).
    /// 3. `shares = floor(shares_issued * deposit / ceil(aum))`, or the
    ///    deposit itself for the first one (`get_shares_to_mint`, `:1069`);
    ///    error `AumZero` when shares are issued but the AUM is zero.
    /// 4. Tokens taken: `ceil(aum * shares / shares_issued)`, or the shares
    ///    for the first deposit
    ///    (`compute_amount_to_deposit_from_shares_to_mint`, `:1294`).
    /// 5. Error `BelowMinimumDeposit` when those tokens are below
    ///    `min_deposit_amount` (`DepositCapReached` if step 2 cut the
    ///    deposit), `DepositMintsNoShares` when no share is minted.
    pub fn deposit(&self, amount: u64) -> Result<DepositOutcome> {
        let deposit = amount
            .checked_sub(self.crank_funds)
            .ok_or(VaultError::BelowCrankFunds {
                amount,
                crank_funds: self.crank_funds,
            })?;
        let aum_sf = self.aum_sf()?;
        let aum_ceil = price::ceil_u64(aum_sf, "vault aum");
        // `try_to_ceil().unwrap_or(u64::MAX)` in the program.
        let cap_aum = aum_ceil.as_ref().map_or(u64::MAX, |ceil| *ceil);
        let cap = if self.deposit_cap == 0 {
            u64::MAX
        } else {
            self.deposit_cap
        };
        let room = cap.saturating_sub(cap_aum);
        if room == 0 {
            return Err(VaultError::DepositCapReached { cap, aum: cap_aum }.into());
        }
        let capped = deposit > room;
        let deposit = deposit.min(room);
        let shares = self.shares_to_mint(aum_sf, aum_ceil, deposit)?;
        let tokens = self.tokens_for_shares_at(aum_sf, shares)?;
        if tokens < self.min_deposit_amount {
            return Err(if capped {
                VaultError::DepositCapReached { cap, aum: cap_aum }
            } else {
                VaultError::BelowMinimumDeposit {
                    amount: tokens,
                    minimum: self.min_deposit_amount,
                }
            }
            .into());
        }
        if shares == 0 {
            return Err(VaultError::DepositMintsNoShares { amount }.into());
        }
        Ok(DepositOutcome {
            tokens: tokens
                .checked_add(self.crank_funds)
                .ok_or_else(|| overflow("vault deposit"))?,
            shares,
            after: Self {
                token_available: self
                    .token_available
                    .checked_add(tokens)
                    .ok_or_else(|| overflow("vault deposit"))?,
                shares_issued: self
                    .shares_issued
                    .checked_add(shares)
                    .ok_or_else(|| overflow("vault deposit"))?,
                ..self.clone()
            },
        })
    }

    /// The shares a deposit of `amount` tokens mints at the current AUM, with
    /// nothing the program applies around the share math: no crank funds, no
    /// deposit cap and no minimum deposit. This is the vault price a deposit
    /// quote uses; the maker pays the shares from its shielded inventory, so
    /// the vault's deposit limits do not apply to the user's swap (they apply
    /// to the maker's rebalance, previewed by [`VaultState::deposit`]).
    ///
    /// `floor(shares_issued * amount / ceil(aum))`, or `amount` for the first
    /// deposit (`get_shares_to_mint`, `vault_operations.rs:1069`). Errors with
    /// `AumBelowPendingFees` from [`VaultState::aum_sf`], `AumZero` when
    /// shares are issued but the AUM is zero (`:1079`), and `Overflow`.
    pub fn shares_for_deposit(&self, amount: u64) -> Result<u64, VaultError> {
        let aum_sf = self.aum_sf()?;
        self.shares_to_mint(aum_sf, price::ceil_u64(aum_sf, "vault aum"), amount)
    }

    /// The tokens `shares` are worth at the current AUM, rounded up: the
    /// pure AUM ratio `ceil(shares * aum / shares_issued)`, or `shares` for
    /// an empty vault, which mints one share per token. No withdrawal
    /// penalty, minimum or liquidity limit applies: this is the deposit that
    /// buys `shares` (`compute_amount_to_deposit_from_shares_to_mint`,
    /// `vault_operations.rs:1294`), not what a withdrawal of them pays.
    ///
    /// Errors with `AumBelowPendingFees` from [`VaultState::aum_sf`],
    /// `AumZero` when shares are issued but the AUM is zero, and `Overflow`.
    pub fn tokens_for_shares(&self, shares: u64) -> Result<u64, VaultError> {
        let aum_sf = self.aum_sf()?;
        if self.shares_issued > 0 && aum_sf == 0 {
            return Err(VaultError::AumZero);
        }
        self.tokens_for_shares_at(aum_sf, shares)
    }

    /// `get_shares_to_mint` (`vault_operations.rs:1069`): `amount` for the
    /// first deposit, else `AumZero` on a zero AUM (`:1079`), else
    /// `Fraction::from(shares_issued).full_mul_int_ratio(amount,
    /// aum.to_ceil())`, rounded down, then `to_floor`. `aum_ceil` is
    /// `ceil(aum_sf)`, computed once by the caller; its overflow only fails
    /// the call when shares are issued, where the program would panic.
    fn shares_to_mint(
        &self,
        aum_sf: u128,
        aum_ceil: Result<u64, VaultError>,
        amount: u64,
    ) -> Result<u64, VaultError> {
        if self.shares_issued == 0 {
            return Ok(amount);
        }
        if aum_sf == 0 {
            return Err(VaultError::AumZero);
        }
        let shares_sf = price::mul_div(
            price::sf(self.shares_issued),
            u128::from(amount),
            u128::from(aum_ceil?),
            Rounding::Down,
        )
        .ok_or_else(|| overflow("shares to mint"))?;
        price::floor_u64(shares_sf, "shares to mint")
    }

    /// `compute_amount_to_deposit_from_shares_to_mint`
    /// (`vault_operations.rs:1294`): `shares` for the first deposit, else
    /// `aum.full_mul_int_ratio_ceil(shares, shares_issued)`, rounded up, then
    /// `to_ceil`.
    fn tokens_for_shares_at(&self, aum_sf: u128, shares: u64) -> Result<u64, VaultError> {
        if self.shares_issued == 0 {
            return Ok(shares);
        }
        let tokens_sf = price::mul_div(
            aum_sf,
            u128::from(shares),
            u128::from(self.shares_issued),
            Rounding::Up,
        )
        .ok_or_else(|| overflow("tokens to deposit"))?;
        price::ceil_u64(tokens_sf, "tokens to deposit")
    }

    /// The withdrawal penalty on `tokens`, rounded up:
    /// `ceil(max(tokens * bps / 10_000, lamports))` with the larger of the
    /// global and the vault bps and lamports (`get_withdrawal_penalty`,
    /// `vault_operations.rs:1239`, `calculate_withdrawal_penalty_params`,
    /// `:1277`). The bps product rounds down before the `max`.
    pub fn withdrawal_penalty(&self, tokens: u64) -> Result<u64, VaultError> {
        let bps = self
            .global_withdrawal_penalty_bps
            .max(self.withdrawal_penalty_bps);
        let lamports = self
            .global_withdrawal_penalty_lamports
            .max(self.withdrawal_penalty_lamports);
        let from_bps_sf = price::mul_div(
            price::sf(tokens),
            u128::from(bps),
            u128::from(FULL_BPS),
            Rounding::Down,
        )
        .ok_or_else(|| overflow("withdrawal penalty"))?;
        price::ceil_u64(from_bps_sf.max(price::sf(lamports)), "withdrawal penalty")
    }

    /// Where a withdrawal paying `tokens` (net of the penalty) draws from,
    /// the one choice both the [`VaultState::withdraw`] preview and the
    /// market maker's withdraw instruction make, so they cannot diverge:
    ///
    /// - `Available` when `token_available >= tokens`: the vault pays from
    ///   its uninvested tokens and `withdraw_from_available` suffices;
    /// - otherwise `Reserve` with the allocated reserve holding the most
    ///   invested liquidity (`invested_liquidity_sf`, the first one on a
    ///   tie): a `withdraw` against it pays `token_available` and
    ///   disinvests the rest from that reserve (`vault_operations.rs:223-230`).
    ///
    /// Errors with `WithdrawExceedsLiquidity` when `token_available` plus
    /// that reserve's liquidity cannot cover `tokens`, since one instruction
    /// disinvests from at most one reserve.
    pub fn withdraw_source(&self, tokens: u64) -> Result<WithdrawSource, VaultError> {
        // `None` or `Some(0)` exactly when `token_available >= tokens`.
        let Some(from_reserve) = tokens
            .checked_sub(self.token_available)
            .filter(|short| *short > 0)
        else {
            return Ok(WithdrawSource::Available);
        };
        let largest = self.reserves.as_slice().iter().reduce(|largest, snapshot| {
            if snapshot.invested_liquidity_sf > largest.invested_liquidity_sf {
                snapshot
            } else {
                largest
            }
        });
        match largest {
            Some(snapshot) if snapshot.invested_liquidity_sf >= price::sf(from_reserve) => {
                Ok(WithdrawSource::Reserve(*snapshot))
            }
            _ => Err(VaultError::WithdrawExceedsLiquidity {
                tokens,
                available: self.token_available.saturating_add(price::floor_u64(
                    largest.map_or(0, |snapshot| snapshot.invested_liquidity_sf),
                    "reserve liquidity",
                )?),
            }),
        }
    }

    /// The withdrawal of `shares`, following kvault `withdraw`
    /// (`vault_operations.rs:175-345`):
    ///
    /// 0. Error `WithdrawZeroShares` for zero shares, `SharesExceedIssued`
    ///    for more than `shares_issued`, `AumBelowPendingFees` from
    ///    [`VaultState::aum_sf`].
    /// 1. `tokens = floor(aum * shares / shares_issued)`, the full AUM for
    ///    all shares (`compute_user_total_received_on_withdraw`, `:1202`);
    ///    errors `AumZero`, `WithdrawPaysNothing`.
    /// 2. `penalty` from [`VaultState::withdrawal_penalty`]; error
    ///    `PenaltyExceedsWithdrawal` when `penalty >= tokens`. The user gets
    ///    `tokens - penalty`; the penalty stays in the vault.
    /// 3. That amount comes from `token_available` first, the rest from the
    ///    reserve [`VaultState::withdraw_source`] picks (the one the maker's
    ///    withdraw instruction names); error `WithdrawExceedsLiquidity` if
    ///    the two cannot cover it.
    /// 4. Shares burned: `ceil(tokens * shares_issued / aum)`, at most
    ///    `shares` (`calculate_shares_to_burn`, `:1320`); error
    ///    `WithdrawBurnsNoShares`.
    /// 5. Error `BelowMinimumWithdraw` when the paid amount is not above
    ///    `min_withdraw_amount` (`:319`).
    ///
    /// Not simulated: the at most one token the program keeps back when a
    /// reserve's cToken conversion rounds (`:264`), and the cToken side of
    /// `after` (`reserves` lose the paid liquidity, `ctoken_allocation` and
    /// `allocations` are unchanged).
    pub fn withdraw(&self, shares: u64) -> Result<WithdrawOutcome> {
        if shares == 0 {
            return Err(VaultError::WithdrawZeroShares.into());
        }
        if shares > self.shares_issued {
            return Err(VaultError::SharesExceedIssued {
                shares,
                issued: self.shares_issued,
            }
            .into());
        }
        let aum_sf = self.aum_sf()?;
        if aum_sf == 0 {
            return Err(VaultError::AumZero.into());
        }
        let gross_sf = if shares == self.shares_issued {
            aum_sf
        } else {
            // `aum.full_mul_int_ratio(shares, shares_issued)`, rounded down.
            price::mul_div(
                aum_sf,
                u128::from(shares),
                u128::from(self.shares_issued),
                Rounding::Down,
            )
            .ok_or_else(|| overflow("vault withdraw"))?
        };
        let gross = price::floor_u64(gross_sf, "vault withdraw")?;
        if gross == 0 {
            return Err(VaultError::WithdrawPaysNothing { shares }.into());
        }
        let penalty = self.withdrawal_penalty(gross)?;
        // `None` or `Some(0)` exactly when `penalty >= gross`.
        let tokens = gross
            .checked_sub(penalty)
            .filter(|tokens| *tokens > 0)
            .ok_or(VaultError::PenaltyExceedsWithdrawal {
                tokens: gross,
                penalty,
            })?;
        let from_available = self.token_available.min(tokens);
        let mut after = self.clone();
        if let WithdrawSource::Reserve(source) = self.withdraw_source(tokens)? {
            let paid_sf = price::sf(
                tokens
                    .checked_sub(from_available)
                    .ok_or_else(|| overflow("withdraw from reserve"))?,
            );
            let snapshot = after
                .reserves
                .as_mut_slice()
                .iter_mut()
                .find(|snapshot| snapshot.reserve == source.reserve)
                .ok_or_else(|| overflow("withdraw source reserve"))?;
            snapshot.invested_liquidity_sf = source
                .invested_liquidity_sf
                .checked_sub(paid_sf)
                .ok_or_else(|| overflow("withdraw source reserve"))?;
        }
        // `full_mul_fraction_ratio_ceil(available + penalty + invested,
        // shares_issued, aum)`, rounded up, then `to_ceil`; the sum is the
        // gross amount when the reserve covers the rest.
        let burned_sf = price::mul_div(
            price::sf(gross),
            price::sf(self.shares_issued),
            aum_sf,
            Rounding::Up,
        )
        .ok_or_else(|| overflow("shares to burn"))?;
        let burned = price::ceil_u64(burned_sf, "shares to burn")?.min(shares);
        if burned == 0 {
            return Err(VaultError::WithdrawBurnsNoShares { shares }.into());
        }
        if tokens <= self.min_withdraw_amount {
            return Err(VaultError::BelowMinimumWithdraw {
                tokens,
                minimum: self.min_withdraw_amount,
            }
            .into());
        }
        after.token_available = after
            .token_available
            .checked_sub(from_available)
            .ok_or_else(|| overflow("vault withdraw"))?;
        after.shares_issued = after
            .shares_issued
            .checked_sub(burned)
            .ok_or_else(|| overflow("vault withdraw"))?;
        Ok(WithdrawOutcome {
            tokens,
            penalty,
            shares: burned,
            after,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An uninvested, fee-free vault with no limits.
    fn vault(token_available: u64, shares_issued: u64) -> VaultState {
        VaultState {
            token_mint: Address::default(),
            token_program: Address::default(),
            token_available,
            shares_issued,
            pending_fees_sf: 0,
            reserves: Reserves::EMPTY,
            min_deposit_amount: 0,
            min_withdraw_amount: 0,
            deposit_cap: 0,
            crank_funds: 0,
            withdrawal_penalty_lamports: 0,
            withdrawal_penalty_bps: 0,
            global_withdrawal_penalty_lamports: 0,
            global_withdrawal_penalty_bps: 0,
        }
    }

    fn vault_error(error: anyhow::Error) -> Option<VaultError> {
        error.downcast_ref::<VaultError>().cloned()
    }

    /// A vault holding `token_available` plus one reserve allocation of
    /// 1_000_000 cTokens in a reserve whose liquidity is twice its
    /// collateral supply (klend `state/reserve.rs:1355`, `:1424`).
    fn invested_vault(token_available: u64, shares_issued: u64) -> VaultState {
        let reserve = kvault::ReserveView {
            lending_market: Address::new_unique(),
            liquidity_supply_vault: Address::default(),
            collateral_mint: Address::default(),
            total_available_amount: 6_000_000,
            borrowed_amount_sf: price::sf(4_000_000),
            accumulated_protocol_fees_sf: 0,
            accumulated_referrer_fees_sf: 0,
            pending_referrer_fees_sf: 0,
            collateral_mint_total_supply: 5_000_000,
        };
        let mut state = vault(token_available, shares_issued);
        let pushed = state.reserves.push(ReserveSnapshot {
            reserve: Address::new_unique(),
            lending_market: reserve.lending_market,
            liquidity_supply_vault: reserve.liquidity_supply_vault,
            collateral_mint: reserve.collateral_mint,
            ctoken_allocation: 1_000_000,
            invested_liquidity_sf: price::collateral_to_liquidity_sf(&reserve, 1_000_000)
                .unwrap_or_default(),
        });
        assert_eq!(pushed, Ok(()));
        state
    }

    /// An uninvested, fee-free vault prices like the share math before
    /// reserves were modelled: `get_shares_to_mint` (`vault_operations.rs:1069`),
    /// `compute_amount_to_deposit_from_shares_to_mint` (`:1294`),
    /// `compute_user_total_received_on_withdraw` (`:1202`) and
    /// `calculate_shares_to_burn` (`:1320`) on `aum = token_available`.
    #[test]
    fn uninvested_vault_reproduces_the_plain_share_math() {
        let state = vault(1_000_003, 999_999);
        let deposit = state.deposit(10_000).ok();
        // shares = floor(999_999 * 10_000 / 1_000_003) = 9_999;
        // tokens = ceil(1_000_003 * 9_999 / 999_999) = 10_000.
        assert_eq!(
            deposit,
            Some(DepositOutcome {
                tokens: 10_000,
                shares: 9_999,
                after: VaultState {
                    token_available: 1_010_003,
                    shares_issued: 1_009_998,
                    ..state.clone()
                },
            })
        );
        // tokens = floor(1_000_003 * 10_000 / 999_999) = 10_000;
        // burned = ceil(10_000 * 999_999 / 1_000_003) = 10_000.
        let withdraw = state.withdraw(10_000).ok();
        assert_eq!(
            withdraw,
            Some(WithdrawOutcome {
                tokens: 10_000,
                penalty: 0,
                shares: 10_000,
                after: VaultState {
                    token_available: 990_003,
                    shares_issued: 989_999,
                    ..state.clone()
                },
            })
        );
        // The first deposit mints one share per token.
        assert_eq!(
            vault(0, 0)
                .deposit(500)
                .map(|outcome| (outcome.tokens, outcome.shares))
                .ok(),
            Some((500, 500))
        );
        // Withdrawing every share pays the full AUM.
        assert_eq!(
            state.withdraw(999_999).map(|outcome| outcome.tokens).ok(),
            Some(1_000_003)
        );
    }

    /// `amounts_invested` (`vault_operations.rs:1124`) and `compute_aum`
    /// (`state.rs:256`): 1_000_000 cTokens at a rate of 2 liquidity per
    /// collateral add 2_000_000 to the AUM.
    #[test]
    fn allocation_adds_its_liquidity_value_to_the_aum() {
        let state = invested_vault(1_000_000, 3_000_000);
        assert_eq!(state.aum_sf(), Ok(price::sf(3_000_000)));
        // One token per share: 300 tokens mint 300 shares.
        assert_eq!(
            state.deposit(300).map(|outcome| outcome.shares).ok(),
            Some(300)
        );
        // 1_500_000 shares pay 1_500_000: 1_000_000 from `token_available`,
        // 500_000 from the reserve.
        let withdraw = state.withdraw(1_500_000).ok();
        let after_reserve = withdraw
            .as_ref()
            .and_then(|outcome| outcome.after.reserves.as_slice().first().copied())
            .map(|snapshot| snapshot.invested_liquidity_sf);
        assert_eq!(
            withdraw.map(|outcome| (
                outcome.tokens,
                outcome.shares,
                outcome.after.token_available
            )),
            Some((1_500_000, 1_500_000, 0))
        );
        assert_eq!(after_reserve, Some(price::sf(1_500_000)));
    }

    /// One withdraw instruction draws from `token_available` and one reserve
    /// (`vault_operations.rs:223-230`): an amount above `token_available`
    /// plus the largest reserve fails with `WithdrawExceedsLiquidity`.
    #[test]
    fn withdrawal_is_limited_to_available_plus_one_reserve() {
        let mut state = invested_vault(1_000_000, 4_000_000);
        let pushed = state.reserves.push(ReserveSnapshot {
            reserve: Address::new_unique(),
            lending_market: Address::new_unique(),
            liquidity_supply_vault: Address::new_unique(),
            collateral_mint: Address::new_unique(),
            ctoken_allocation: 1_000_000,
            invested_liquidity_sf: price::sf(1_000_000),
        });
        assert_eq!(pushed, Ok(()));
        assert_eq!(state.aum_sf(), Ok(price::sf(4_000_000)));
        assert_eq!(
            state.withdraw(3_000_000).map(|outcome| outcome.tokens).ok(),
            Some(3_000_000)
        );
        assert_eq!(
            state.withdraw(3_000_001).err().and_then(vault_error),
            Some(VaultError::WithdrawExceedsLiquidity {
                tokens: 3_000_001,
                available: 3_000_000,
            })
        );
    }

    /// `withdraw_source` pays from `token_available` while it covers the
    /// amount, then names the reserve with the most invested liquidity.
    #[test]
    fn withdraw_source_prefers_available_then_the_largest_reserve() {
        let mut state = invested_vault(1_000_000, 4_000_000);
        let smaller = ReserveSnapshot {
            reserve: Address::new_unique(),
            lending_market: Address::new_unique(),
            liquidity_supply_vault: Address::new_unique(),
            collateral_mint: Address::new_unique(),
            ctoken_allocation: 1_000_000,
            invested_liquidity_sf: price::sf(1_000_000),
        };
        assert_eq!(state.reserves.push(smaller), Ok(()));
        let largest = state.reserves.as_slice().first().copied();
        assert_eq!(
            state.withdraw_source(1_000_000),
            Ok(WithdrawSource::Available)
        );
        assert_eq!(
            state.withdraw_source(1_000_001).ok(),
            largest.map(WithdrawSource::Reserve)
        );
        assert_eq!(
            state.withdraw_source(3_000_001),
            Err(VaultError::WithdrawExceedsLiquidity {
                tokens: 3_000_001,
                available: 3_000_000,
            })
        );
        assert_eq!(
            vault(10, 10).withdraw_source(11),
            Err(VaultError::WithdrawExceedsLiquidity {
                tokens: 11,
                available: 10,
            })
        );
    }

    /// `compute_aum` (`state.rs:256-266`): pending fees come off the AUM, and
    /// fees above the holdings fail with `AUMBelowPendingFees`.
    #[test]
    fn pending_fees_reduce_the_aum() {
        let mut state = vault(1_000, 1_000);
        state.pending_fees_sf = price::sf(200);
        assert_eq!(state.aum_sf(), Ok(price::sf(800)));
        // 100 shares of 1_000 at an AUM of 800 pay 80.
        assert_eq!(
            state.withdraw(100).map(|outcome| outcome.tokens).ok(),
            Some(80)
        );
        // 80 tokens mint floor(1_000 * 80 / 800) = 100 shares.
        assert_eq!(
            state.deposit(80).map(|outcome| outcome.shares).ok(),
            Some(100)
        );

        state.pending_fees_sf = price::sf(1_000) + 1;
        assert_eq!(
            state.deposit(80).err().and_then(vault_error),
            Some(VaultError::AumBelowPendingFees {
                holdings_sf: price::sf(1_000),
                pending_fees_sf: price::sf(1_000) + 1,
            })
        );
    }

    /// `get_withdrawal_penalty` (`vault_operations.rs:1239`) with
    /// `calculate_withdrawal_penalty_params` (`:1277`): the larger of global
    /// and vault settings each, then the larger of the bps and lamport
    /// penalties, rounded up; `WithdrawAmountLessThanWithdrawalPenalty`
    /// (`:216`) when the penalty takes everything.
    #[test]
    fn withdrawal_penalty_takes_the_larger_branch() {
        let mut state = vault(1_000_000, 1_000_000);
        state.withdrawal_penalty_bps = 10;
        state.global_withdrawal_penalty_bps = 25;
        state.withdrawal_penalty_lamports = 3;
        state.global_withdrawal_penalty_lamports = 1;
        // bps branch: ceil(10_001 * 25 / 10_000) = ceil(25.0025) = 26 > 3.
        let withdraw = state.withdraw(10_001).ok();
        assert_eq!(
            withdraw
                .as_ref()
                .map(|outcome| (outcome.tokens, outcome.penalty, outcome.shares)),
            Some((9_975, 26, 10_001))
        );
        assert_eq!(
            withdraw.map(|outcome| outcome.after.token_available),
            Some(1_000_000 - 9_975)
        );
        // lamports branch: 100 * 25 / 10_000 = 0.25 < 3.
        assert_eq!(
            state
                .withdraw(100)
                .map(|outcome| (outcome.tokens, outcome.penalty))
                .ok(),
            Some((97, 3))
        );
        // The penalty of 3 takes all of 3 tokens.
        assert_eq!(
            state.withdraw(3).err().and_then(vault_error),
            Some(VaultError::PenaltyExceedsWithdrawal {
                tokens: 3,
                penalty: 3,
            })
        );
    }

    /// `deposit` (`vault_operations.rs:80-115`) with
    /// `get_max_depositable_in_vault` (`:962`): no room under the cap fails
    /// with `VaultDepositCapReached`, a larger deposit is cut to the room,
    /// and a deposit below `min_deposit_amount` fails with
    /// `DepositAmountBelowMinimum`.
    #[test]
    fn deposit_cap_and_minimum_bound_the_deposit() {
        let mut state = vault(1_000, 1_000);
        state.deposit_cap = 1_000;
        assert_eq!(
            state.deposit(10).err().and_then(vault_error),
            Some(VaultError::DepositCapReached {
                cap: 1_000,
                aum: 1_000,
            })
        );
        state.deposit_cap = 1_050;
        assert_eq!(
            state.deposit(100).map(|outcome| outcome.tokens).ok(),
            Some(50)
        );
        state.min_deposit_amount = 60;
        assert_eq!(
            state.deposit(100).err().and_then(vault_error),
            Some(VaultError::DepositCapReached {
                cap: 1_050,
                aum: 1_000,
            })
        );
        state.deposit_cap = 0;
        assert_eq!(
            state.deposit(59).err().and_then(vault_error),
            Some(VaultError::BelowMinimumDeposit {
                amount: 59,
                minimum: 60,
            })
        );
        assert_eq!(
            state.deposit(60).map(|outcome| outcome.shares).ok(),
            Some(60)
        );
    }

    /// `deposit` (`vault_operations.rs:66-68`, `handler_deposit.rs:70`): the
    /// crank funds come off the deposited amount and are taken on top.
    #[test]
    fn crank_funds_are_taken_on_top_of_the_deposit() {
        let mut state = vault(1_000, 1_000);
        state.crank_funds = 7;
        assert_eq!(
            state
                .deposit(107)
                .map(|outcome| (
                    outcome.tokens,
                    outcome.shares,
                    outcome.after.token_available
                ))
                .ok(),
            Some((107, 100, 1_100))
        );
        assert_eq!(
            state.deposit(6).err().and_then(vault_error),
            Some(VaultError::BelowCrankFunds {
                amount: 6,
                crank_funds: 7,
            })
        );
    }

    /// `shares_for_deposit` is the share math of `get_shares_to_mint`
    /// (`vault_operations.rs:1069`) alone: the crank funds, the deposit cap
    /// and the minimum that `deposit` applies do not change it.
    #[test]
    fn shares_for_deposit_ignores_crank_cap_and_minimum() {
        let mut state = vault(1_000_003, 999_999);
        // floor(999_999 * 10_000 / 1_000_003) = 9_999, as in `deposit`.
        assert_eq!(state.shares_for_deposit(10_000), Ok(9_999));
        state.crank_funds = 7;
        state.deposit_cap = 1_000_050;
        state.min_deposit_amount = 1_000_000;
        assert_eq!(state.shares_for_deposit(10_000), Ok(9_999));
        for (label, state, want) in [
            (
                "first deposit mints one share per token",
                vault(0, 0),
                Ok(500),
            ),
            (
                "shares issued against a zero AUM",
                vault(0, 10),
                Err(VaultError::AumZero),
            ),
        ] {
            let got = state.shares_for_deposit(500);
            assert_eq!(got, want, "{label}: got {got:?}, want {want:?}");
        }
    }

    /// `tokens_for_shares` is `ceil(shares * aum / shares_issued)`
    /// (`compute_amount_to_deposit_from_shares_to_mint`,
    /// `vault_operations.rs:1294`) with no penalty, minimum or issued-shares
    /// limit: it prices more shares than the vault has issued, where a
    /// withdrawal of them fails.
    #[test]
    fn tokens_for_shares_is_the_pure_aum_ratio() {
        // 3 tokens per 2 shares: 3 shares are worth ceil(4.5) = 5 tokens.
        let mut state = vault(3_000, 2_000);
        assert_eq!(state.tokens_for_shares(3), Ok(5));
        state.withdrawal_penalty_bps = 5_000;
        state.min_withdraw_amount = 1_000;
        assert_eq!(state.tokens_for_shares(3), Ok(5));
        // 2 tokens per share, all invested in one reserve.
        let invested = invested_vault(0, 1_000_000);
        assert_eq!(invested.tokens_for_shares(1_500_000), Ok(3_000_000));
        assert_eq!(
            invested.withdraw(1_500_000).err().and_then(vault_error),
            Some(VaultError::SharesExceedIssued {
                shares: 1_500_000,
                issued: 1_000_000,
            })
        );
        for (label, state, want) in [
            (
                "an empty vault mints one share per token",
                vault(0, 0),
                Ok(7),
            ),
            (
                "shares issued against a zero AUM",
                vault(0, 10),
                Err(VaultError::AumZero),
            ),
        ] {
            let got = state.tokens_for_shares(7);
            assert_eq!(got, want, "{label}: got {got:?}, want {want:?}");
        }
    }

    /// `withdraw` (`vault_operations.rs:319`): the paid amount must be above
    /// `min_withdraw_amount`.
    #[test]
    fn withdrawal_must_exceed_the_minimum() {
        let mut state = vault(1_000, 1_000);
        state.min_withdraw_amount = 10;
        assert_eq!(
            state.withdraw(10).err().and_then(vault_error),
            Some(VaultError::BelowMinimumWithdraw {
                tokens: 10,
                minimum: 10,
            })
        );
        assert_eq!(
            state.withdraw(11).map(|outcome| outcome.tokens).ok(),
            Some(11)
        );
    }
}
