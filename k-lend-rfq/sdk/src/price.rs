//! Fixed-point arithmetic of the kVault and klend programs, on raw bits.
//!
//! Both programs keep amounts as `Fraction = fixed::types::U68F60` (klend
//! `utils/fraction.rs:2`, re-exported as `kvault_interface::Fraction`): a
//! `u128` whose low 60 bits are the fractional part. Account fields store the
//! raw bits under an `_sf` ("scaled fraction") suffix. This module works on
//! those raw bits and reproduces the rounding of each program operation it
//! ports; the 256-bit intermediates the programs compute with `uint::U256`
//! are computed by [`mul_div`].

use crate::{
    kvault::ReserveView,
    pair::{overflow, VaultError},
};

/// Fractional bits of `U68F60`.
const FRACTION_BITS: u32 = 60;
/// `Fraction::ONE` as raw bits (klend `utils/fraction.rs` `FRACTION_ONE_SCALED`).
pub(crate) const ONE_SF: u128 = 1 << FRACTION_BITS;

/// Rounding direction of a division.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Rounding {
    Down,
    Up,
}

/// `Fraction::from(int)`: exact, a `u64` always fits the 68 integer bits.
pub(crate) fn sf(int: u64) -> u128 {
    u128::from(int) << FRACTION_BITS
}

/// `Fraction::to_floor::<u64>()`; errors where the program would panic.
pub(crate) fn floor_u64(value_sf: u128, context: &'static str) -> Result<u64, VaultError> {
    u64::try_from(value_sf >> FRACTION_BITS).map_err(|_| overflow(context))
}

/// `Fraction::to_ceil::<u64>()`; errors where the program would panic.
pub(crate) fn ceil_u64(value_sf: u128, context: &'static str) -> Result<u64, VaultError> {
    u64::try_from(value_sf.div_ceil(ONE_SF)).map_err(|_| overflow(context))
}

/// `a * b / divisor` with a 256-bit product, rounded as requested.
///
/// This is the `U256` step of klend `full_mul_int_ratio` (floor),
/// `full_mul_int_ratio_ceil` (ceil) and kvault `full_mul_fraction_ratio_ceil`
/// (ceil). `None` on a zero divisor or a quotient above `u128::MAX`, where the
/// programs panic.
pub(crate) fn mul_div(a: u128, b: u128, divisor: u128, rounding: Rounding) -> Option<u128> {
    if divisor == 0 {
        return None;
    }
    let (high, low) = widening_mul(a, b);
    // A quotient fits in 128 bits exactly when the high half is below the
    // divisor.
    if high >= divisor {
        return None;
    }
    // Restoring long division of `high:low` by `divisor`, one bit of `low` at
    // a time. `remainder < divisor` holds before every step, so the shifted
    // remainder is below `2 * divisor` and one subtraction restores it.
    let mut remainder = high;
    let mut quotient = 0u128;
    for bit in (0..u128::BITS).rev() {
        let carry = remainder >> (u128::BITS - 1);
        remainder = (remainder << 1) | ((low >> bit) & 1);
        quotient <<= 1;
        if carry == 1 || remainder >= divisor {
            // With `carry` set the true remainder is `2^128 + remainder`;
            // the wrapping subtraction yields the exact result below
            // `divisor`.
            remainder = remainder.wrapping_sub(divisor);
            quotient |= 1;
        }
    }
    match rounding {
        Rounding::Up if remainder != 0 => quotient.checked_add(1),
        Rounding::Down | Rounding::Up => Some(quotient),
    }
}

/// The full 256-bit product of two `u128`s as `(high, low)` halves.
fn widening_mul(a: u128, b: u128) -> (u128, u128) {
    const MASK: u128 = u64::MAX as u128;
    let (a_high, a_low) = (a >> 64, a & MASK);
    let (b_high, b_low) = (b >> 64, b & MASK);
    // Each partial product is below 2^128.
    let low_low = a_low * b_low;
    let low_high = a_low * b_high;
    let high_low = a_high * b_low;
    let high_high = a_high * b_high;
    // Sum of three values below 2^64 each: below 2^66.
    let middle = (low_low >> 64) + (low_high & MASK) + (high_low & MASK);
    let low = (low_low & MASK) | ((middle & MASK) << 64);
    // The full product is below 2^256, so this sum cannot overflow.
    let high = high_high + (low_high >> 64) + (high_low >> 64) + (middle >> 64);
    (high, low)
}

/// The liquidity value, as `Fraction` bits, of `collateral` cTokens of
/// `reserve`, exactly as kvault `amounts_invested`
/// (`operations/vault_operations.rs:1124`) computes it with klend v1.21.0:
///
/// 1. `ReserveLiquidity::total_supply` (`state/reserve.rs:1113`):
///    `total_available_amount + borrowed_amount_sf - accumulated_protocol_fees_sf
///    - accumulated_referrer_fees_sf - pending_referrer_fees_sf`, exact.
/// 2. `ReserveCollateral::exchange_rate` (`state/reserve.rs:1355`):
///    `INITIAL_COLLATERAL_RATE` (`utils/consts.rs:42`,
///    `CollateralExchangeRate::ONE`: one liquidity per collateral) when the
///    collateral supply or the total liquidity is zero, else
///    `{ collateral_supply: mint_total_supply, liquidity: total_supply }`.
/// 3. `fraction_collateral_to_liquidity` (`state/reserve.rs:1424`):
///    `BigFraction(collateral) * BigFraction(liquidity) / collateral_supply`.
///    The `BigFraction` product is `(collateral_sf * liquidity_sf) >> 60`,
///    which is exactly `collateral * liquidity_sf`; the integer division
///    rounds down.
pub(crate) fn collateral_to_liquidity_sf(
    reserve: &ReserveView,
    collateral: u64,
) -> Result<u128, VaultError> {
    let total_supply_sf = sf(reserve.total_available_amount)
        .checked_add(reserve.borrowed_amount_sf)
        .and_then(|sum| sum.checked_sub(reserve.accumulated_protocol_fees_sf))
        .and_then(|sum| sum.checked_sub(reserve.accumulated_referrer_fees_sf))
        .and_then(|sum| sum.checked_sub(reserve.pending_referrer_fees_sf))
        .ok_or_else(|| overflow("reserve total supply"))?;
    let (collateral_supply, liquidity_sf) =
        if reserve.collateral_mint_total_supply == 0 || total_supply_sf == 0 {
            (1, ONE_SF)
        } else {
            (
                u128::from(reserve.collateral_mint_total_supply),
                total_supply_sf,
            )
        };
    mul_div(
        u128::from(collateral),
        liquidity_sf,
        collateral_supply,
        Rounding::Down,
    )
    .ok_or_else(|| overflow("collateral to liquidity"))
}

#[cfg(test)]
mod tests {
    use solana_address::Address;

    use super::*;

    fn reserve(total_available_amount: u64, collateral_mint_total_supply: u64) -> ReserveView {
        ReserveView {
            lending_market: Address::default(),
            liquidity_supply_vault: Address::default(),
            collateral_mint: Address::default(),
            total_available_amount,
            borrowed_amount_sf: 0,
            accumulated_protocol_fees_sf: 0,
            accumulated_referrer_fees_sf: 0,
            pending_referrer_fees_sf: 0,
            collateral_mint_total_supply,
        }
    }

    /// `mul_div` returns `a * b / divisor` rounded as asked, and `None` for a
    /// zero divisor.
    #[test]
    fn mul_div_rounds_as_asked() {
        for (label, a, b, divisor, rounding, want) in [
            ("35 / 3 down", 7, 5, 3, Rounding::Down, Some(11)),
            ("35 / 3 up", 7, 5, 3, Rounding::Up, Some(12)),
            ("exact 30 / 3 up", 6, 5, 3, Rounding::Up, Some(10)),
            ("zero divisor", 1, 1, 0, Rounding::Down, None),
        ] {
            let got = mul_div(a, b, divisor, rounding);
            assert_eq!(got, want, "{label}: got {got:?}, want {want:?}");
        }
    }

    /// `mul_div` computes the product in 256 bits: a product above
    /// `u128::MAX` divides correctly, and only a quotient above `u128::MAX`
    /// is `None`.
    #[test]
    fn mul_div_uses_a_256_bit_product() {
        for (label, a, b, divisor, rounding, want) in [
            (
                "(2^127 * 2^64) / 2^64: product overflows u128, quotient does not",
                1 << 127,
                1 << 64,
                1 << 64,
                Rounding::Down,
                Some(1 << 127),
            ),
            (
                "MAX * MAX / MAX up",
                u128::MAX,
                u128::MAX,
                u128::MAX,
                Rounding::Up,
                Some(u128::MAX),
            ),
            (
                "(2^128 - 1)^2 / (2^128 - 2) = 2^128 + 1/(2^128 - 2): above MAX",
                u128::MAX,
                u128::MAX,
                u128::MAX - 1,
                Rounding::Down,
                None,
            ),
            (
                "(2^100 + 3) * 2^100 / 2^72 = 2^128 + 3 * 2^28: above MAX",
                (1 << 100) + 3,
                1 << 100,
                1 << 72,
                Rounding::Down,
                None,
            ),
            (
                "(2^100 + 3) * 2^100 / 2^73 = 2^127 + 3 * 2^27",
                (1 << 100) + 3,
                1 << 100,
                1 << 73,
                Rounding::Down,
                Some((1 << 127) + 3 * (1 << 27)),
            ),
        ] {
            let got = mul_div(a, b, divisor, rounding);
            assert_eq!(got, want, "{label}: got {got:?}, want {want:?}");
        }
    }

    /// `floor_u64` drops and `ceil_u64` rounds up a fractional part of a
    /// scaled value; a whole value converts unchanged.
    #[test]
    fn floor_and_ceil_round_the_fraction() {
        let one_and_a_bit = sf(1) + 1;
        for (label, got, want) in [
            ("floor of 1 + 2^-60", floor_u64(one_and_a_bit, "test"), 1),
            ("ceil of 1 + 2^-60", ceil_u64(one_and_a_bit, "test"), 2),
            ("ceil of exactly 1", ceil_u64(sf(1), "test"), 1),
        ] {
            assert_eq!(got, Ok(want), "{label}: got {got:?}, want {want}");
        }
    }

    /// klend `state/reserve.rs:1355`: an empty reserve converts at
    /// `INITIAL_COLLATERAL_RATE`, one liquidity per collateral.
    #[test]
    fn empty_reserve_converts_one_to_one() {
        assert_eq!(collateral_to_liquidity_sf(&reserve(0, 0), 5), Ok(sf(5)));
        assert_eq!(collateral_to_liquidity_sf(&reserve(1_000, 0), 5), Ok(sf(5)));
    }

    /// klend `state/reserve.rs:1113` and `:1424`: borrowed liquidity counts
    /// towards the supply, fees do not, and the result rounds down.
    #[test]
    fn exchange_rate_counts_borrows_and_excludes_fees() {
        let mut view = reserve(2_000, 3_000);
        view.borrowed_amount_sf = sf(4_000) + ONE_SF / 2;
        view.accumulated_protocol_fees_sf = ONE_SF / 4;
        view.accumulated_referrer_fees_sf = ONE_SF / 8;
        view.pending_referrer_fees_sf = ONE_SF / 8;
        // Total supply is exactly 6_000; 1 collateral is worth 2 liquidity.
        assert_eq!(collateral_to_liquidity_sf(&view, 7), Ok(sf(14)));
        // 1 collateral of supply 3 at liquidity 1 is a third: rounds down.
        let third = collateral_to_liquidity_sf(&reserve(1, 3), 1);
        assert_eq!(third, Ok(ONE_SF / 3));
    }
}
