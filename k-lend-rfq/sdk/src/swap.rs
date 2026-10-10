//! The swap protocol's wire types (offer, request, fill), the order id and
//! its on-chain marker, and `SwapError`, the one error type both parties use
//! for every check of the swap.

use std::{fmt, time::Instant};

use anyhow::Result;
use solana_address::Address;
use solana_instruction::Instruction;
use solana_message::VersionedMessage;
use solana_system_interface::{instruction::create_account_with_seed, program as system_program};
use zolana_keypair::ShieldedAddress;

use crate::pair::{Pair, VaultState};

/// One whole in basis points (klend `utils/consts.rs` `FULL_BPS`);
/// `Quote::price` keeps `FULL_BPS - fee_bps` of the gross amount, so a
/// larger fee is rejected.
pub const FULL_BPS: u64 = 10_000;

/// Every way a quote, fill or pre-signing check of a swap fails. The maker's
/// checks and the user's checks share it, so a test or client can match the
/// exact variant whichever side rejected.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SwapError {
    /// The maker's available inventory of `asset` sums to less than the
    /// amount to pay (at quote or at fill).
    #[error(
        "the market maker's largest {asset} utxo holds {available}, the fill needs {required}"
    )]
    InsufficientInventory {
        asset: Address,
        required: u64,
        available: u64,
    },
    #[error(
        "the market maker cannot pay {required} {asset} with at most {max_inputs} inputs next to a user transfer"
    )]
    /// The maker's inventory covers the amount only with more than
    /// `max_inputs` UTXOs, or leaves no room for a user input.
    MakerTransferTooWide {
        asset: Address,
        required: u64,
        max_inputs: usize,
    },
    #[error(
        "the swap would leave {asset} at {balance_after}, outside its target range {min}..={max}"
    )]
    /// The swap would push the maker's net balance of `asset` out of its
    /// configured target range.
    OutsideTargetRange {
        asset: Address,
        balance_after: u64,
        min: u64,
        max: u64,
    },
    /// The user's UTXOs of `asset` sum to less than `required`.
    #[error("the user has no {asset} utxo covering {required}")]
    InsufficientFunds { asset: Address, required: u64 },
    /// A transfer in the swap moves funds to or from a public token account.
    #[error("the swap carries {count} interface transfers")]
    PublicTransfer { count: usize },
    /// The message or instruction does not have the swap's shape: wrong fee
    /// payer, instruction count, program, tag or account index.
    #[error("the transaction is not the user's transfer followed by one maker transact")]
    UnexpectedTransaction,
    /// The first instruction of the swap message differs from the transfer
    /// the user proved.
    #[error("the transaction does not carry the user's transfer as the user built it")]
    UserTransferAltered,
    /// The maker's transfer does not pay the user exactly one output of the
    /// quoted asset.
    #[error("the maker's transfer pays the user {received} outputs, expected one")]
    UnexpectedOutputs { received: usize },
    /// A decrypted output does not hash to the commitment the transaction
    /// publishes for it, so the plaintext cannot be trusted.
    #[error("output {slot} does not open to its commitment")]
    CommitmentMismatch { slot: usize },
    /// The user's outputs to the maker do not sum to exactly the order's
    /// `amount_in` (more is rejected as well).
    #[error("the user's transfer pays the maker {received}, the quote takes {expected}")]
    Underpaid { expected: u64, received: u64 },
    /// The maker's output to the user holds less than the quoted
    /// `amount_out`.
    #[error("the fill pays {offered}, the quote pays {quoted}")]
    BelowQuote { quoted: u64, offered: u64 },
    /// The user's transfer spends more inputs than the offer's
    /// `max_user_inputs`.
    #[error("the user's transfer takes {inputs} inputs, the quote allows {max}")]
    UserTransferTooWide { inputs: usize, max: usize },
    /// The user's transfer does not have the offer's `user_outputs` outputs.
    #[error("the user's transfer has {outputs} outputs, the quote expects {expected}")]
    UserTransferOutputs { outputs: usize, expected: usize },
    /// Covering the amount takes more of the user's UTXOs than the offer
    /// allows; the user consolidates first.
    #[error("the user's balance needs {needed} inputs, the quote allows {max}")]
    TooManyInputs { needed: usize, max: usize },
    /// No proof shape in `SPP_SUPPORTED_SHAPES` is wide enough.
    #[error("no supported shape takes {inputs} inputs and {outputs} outputs")]
    NoSupportedShape { inputs: usize, outputs: usize },
    /// The maker holds no open order `order`: never issued, swept after
    /// expiry, or already consumed by an earlier fill attempt.
    #[error("the market maker issued no open order {order}")]
    UnknownOrder { order: OrderId },
    /// The order's expiry passed before the fill (maker) or before proving
    /// (user).
    #[error("order {order} expired before the fill")]
    OrderExpired { order: OrderId },
    /// The order's marker account already exists on chain.
    #[error("order {order} is already filled")]
    OrderAlreadyFilled { order: OrderId },
    /// The fill names a different pair than the order was quoted for.
    #[error("order {order} was quoted for a different pair")]
    OrderPairMismatch { order: OrderId },
    /// `create_with_seed` rejected the order's marker derivation.
    #[error("order {order} gives no valid marker address")]
    InvalidOrderMarker { order: OrderId },
    /// The swap message's last instruction is not the order's marker
    /// creation as the user rebuilds it.
    #[error("the transaction does not create the marker of order {order}")]
    OrderMarkerMismatch { order: OrderId },
    /// A sum of amounts overflows `u64`; `context` names it.
    #[error("amount overflow in {context}")]
    AmountOverflow { context: &'static str },
    /// `amount_in` prices to zero of the output asset after the fee, so no
    /// order is opened for it.
    #[error("a swap of {amount_in} prices to zero")]
    QuoteZero { amount_in: u64 },
    /// An instruction of the swap message other than the user's transfer
    /// names `signer` as a signer: a user signing key, or for the maker's
    /// transfer any key but the offer's fee payer. The user's signature would
    /// authorize it.
    #[error("the swap message names {signer} as a signer outside the user's transfer")]
    UnexpectedSigner { signer: Address },
}

/// Identifies one offer the market maker issued. The maker draws it at
/// random when it quotes and keeps the quoted amounts under it; at fill time
/// the maker resolves the id against its own record and rejects an id that is
/// unknown, expired, already filled or quoted for another pair.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct OrderId(pub [u8; ORDER_ID_BYTES]);

/// Length of an order id: its hex form, two characters per byte, is the
/// marker seed and must fit the `create_with_seed` seed limit.
pub const ORDER_ID_BYTES: usize = solana_address::MAX_SEED_LEN / 2;

impl OrderId {
    /// A fresh id from the thread RNG; 128 random bits make a collision
    /// between open orders negligible.
    pub fn random() -> Self {
        Self(rand::random())
    }
}

/// The id as lowercase hex, `2 * ORDER_ID_BYTES` = 32 characters, the
/// maximum `create_with_seed` seed length. Used as the marker seed.
impl fmt::Display for OrderId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.iter().try_for_each(|byte| write!(f, "{byte:02x}"))
    }
}

/// The instruction that marks order `id` as filled: a system
/// `create_account_with_seed` from `fee_payer` to the marker address
/// `Address::create_with_seed(fee_payer, &id.to_string(), &system_program::ID)`,
/// with `fee_payer` as the seed base, `id.to_string()` as the seed, `lamports`
/// funding, zero space and the system program as owner.
///
/// Every swap transaction ends with this instruction, so each fill locks the
/// rent minimum of an empty account (`lamports`, paid by the maker as fee
/// payer) at the marker address. The system program rejects the creation of
/// an account that already exists with `SystemError::AccountAlreadyInUse`
/// (custom code 0), so a second transaction filling the same order fails
/// on-chain. Reclaiming the locked lamports after the order expires
/// (`transfer_with_seed`) is a follow-up and not done here.
///
/// Fails with `SwapError::InvalidOrderMarker` if the marker derivation
/// rejects the seed, which a 32 character hex seed and the system program
/// owner never trigger.
pub fn order_marker_instruction(
    fee_payer: &Address,
    id: OrderId,
    lamports: u64,
) -> Result<Instruction, SwapError> {
    let seed = id.to_string();
    let marker = Address::create_with_seed(fee_payer, &seed, &system_program::ID)
        .map_err(|_| SwapError::InvalidOrderMarker { order: id })?;
    Ok(create_account_with_seed(
        fee_payer,
        &marker,
        fee_payer,
        &seed,
        lamports,
        0,
        &system_program::ID,
    ))
}

/// Which way the user swaps through the vault.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// The user pays tokens and receives shares.
    Deposit,
    /// The user pays shares and receives tokens.
    Withdrawal,
}

impl Direction {
    /// `(asset the user pays, asset the user receives)` on `pair`.
    pub fn assets(self, pair: &Pair) -> (Address, Address) {
        match self {
            Self::Deposit => (pair.token_mint, pair.shares_mint),
            Self::Withdrawal => (pair.shares_mint, pair.token_mint),
        }
    }
}

/// A priced swap: the user pays `amount_in` of the direction's input asset
/// and receives `amount_out` of its output asset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Quote {
    pub direction: Direction,
    pub amount_in: u64,
    pub amount_out: u64,
}

impl Quote {
    /// Prices `amount_in` at the vault's price ([`VaultState`]) minus
    /// `fee_bps`.
    ///
    /// - Deposit: the gross is [`VaultState::shares_for_deposit`], the plain
    ///   share math for all of `amount_in` tokens. The vault's crank funds,
    ///   deposit cap and minimum deposit do not apply: the maker pays the
    ///   shares from its inventory, and those limits bound only its own
    ///   rebalance deposit. So the quote prices every token the user pays.
    /// - Withdrawal: the gross is the tokens [`VaultState::withdraw`] pays for
    ///   `amount_in` shares net of the withdrawal penalty, so the maker never
    ///   quotes more collateral than its own vault withdrawal realises.
    ///
    /// `amount_out = floor(gross * (FULL_BPS - fee_bps) / FULL_BPS)`; a
    /// `fee_bps` above `FULL_BPS` quotes zero. Errors with the `VaultError`
    /// the vault math returns for `amount_in`, and with
    /// `SwapError::QuoteZero` when `amount_out` is zero, so no order is
    /// opened that pays the user nothing.
    pub fn price(
        rate: &VaultState,
        direction: Direction,
        amount_in: u64,
        fee_bps: u64,
    ) -> Result<Self> {
        let gross = match direction {
            Direction::Deposit => rate.shares_for_deposit(amount_in)?,
            Direction::Withdrawal => rate.withdraw(amount_in)?.tokens,
        };
        // A `u64` times at most `FULL_BPS` fits a `u128`.
        let amount_out = u64::try_from(
            u128::from(gross) * u128::from(FULL_BPS.saturating_sub(fee_bps)) / u128::from(FULL_BPS),
        )?;
        if amount_out == 0 {
            return Err(SwapError::QuoteZero { amount_in }.into());
        }
        Ok(Self {
            direction,
            amount_in,
            amount_out,
        })
    }
}

/// The maker's answer to a quote request, binding until `expires_at`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Offer {
    /// The order this offer opens. The user names it in its `SwapRequest`;
    /// the maker checks at fill time that it is known, unexpired, unfilled and
    /// quoted for the requested pair.
    pub id: OrderId,
    /// Unix seconds; the maker refuses a fill after this time.
    pub expires_at: u64,
    pub quote: Quote,
    /// The maker's shielded address, the recipient of the user's transfer.
    pub maker: ShieldedAddress,
    /// The maker's public key that pays the swap's fee and signs first.
    pub fee_payer: Address,
    /// Most inputs the user's transfer may spend; the rest of the
    /// transaction is sized for the maker's transfer.
    pub max_user_inputs: usize,
    /// Outputs the user's transfer must have (payment and change).
    pub user_outputs: usize,
    /// The rent minimum of an empty account. The maker funds the order's
    /// marker account (`order_marker_instruction`) with it, and the user
    /// rebuilds the marker instruction from it when checking the swap message.
    pub marker_lamports: u64,
}

/// The user's fill request: the order and the proven transfer that pays it.
pub struct SwapRequest {
    /// The order being filled. The maker takes the amounts from its own record
    /// of this order, never from the request, and rejects the fill if the
    /// order is unknown, expired, already filled or quoted for another pair.
    pub order: OrderId,
    /// The user's shielded address, the recipient of the maker's transfer.
    pub user: ShieldedAddress,
    /// The user's proven zolana `transact` paying `amount_in` to the maker.
    pub transfer: Instruction,
}

/// The user's side of a swap in progress: the offer and the request sent to
/// the maker.
pub struct Order {
    pub offer: Offer,
    pub request: SwapRequest,
}

/// The swap message the maker returns for the user to verify and sign.
pub struct Fill {
    /// Unsigned message: user transfer, maker transfer, order marker.
    pub message: VersionedMessage,
    /// The maker refuses to co-sign at or after this instant.
    pub expires_at: Instant,
}

#[cfg(test)]
mod tests {
    use super::{Direction, OrderId, Quote, SwapError};
    use crate::pair::{Reserves, VaultState};

    /// One token per share, with a deposit cap 50 tokens above the AUM, a
    /// minimum deposit and crank funds: a vault `deposit` of 1_000 would be
    /// cut to 50 tokens.
    fn capped_vault() -> VaultState {
        VaultState {
            token_mint: solana_address::Address::default(),
            token_program: solana_address::Address::default(),
            token_available: 1_000,
            shares_issued: 1_000,
            pending_fees_sf: 0,
            reserves: Reserves::EMPTY,
            min_deposit_amount: 40,
            min_withdraw_amount: 0,
            deposit_cap: 1_050,
            crank_funds: 7,
            withdrawal_penalty_lamports: 0,
            withdrawal_penalty_bps: 0,
            global_withdrawal_penalty_lamports: 0,
            global_withdrawal_penalty_bps: 0,
        }
    }

    /// A deposit quote prices all of `amount_in` at the share math, even
    /// where the vault's own deposit would be cut by the cap and charged the
    /// crank funds.
    #[test]
    fn deposit_quote_prices_the_full_amount_past_the_cap() {
        let vault = capped_vault();
        let capped = vault.deposit(1_000).map(|outcome| outcome.shares).ok();
        assert_eq!(capped, Some(50), "the vault deposit is cut to the cap");
        let quote = Quote::price(&vault, Direction::Deposit, 1_000, 0).ok();
        assert_eq!(
            quote,
            Some(Quote {
                direction: Direction::Deposit,
                amount_in: 1_000,
                amount_out: 1_000,
            })
        );
    }

    /// A quote whose `amount_out` is zero after the fee fails with
    /// `QuoteZero`.
    #[test]
    fn quote_paying_nothing_is_refused() {
        let vault = capped_vault();
        for (label, amount_in, fee_bps) in [
            ("the whole amount is fee", 1_000, 10_000),
            ("one share less 30 bps rounds to zero", 1, 30),
        ] {
            let got = Quote::price(&vault, Direction::Deposit, amount_in, fee_bps)
                .err()
                .and_then(|error| error.downcast_ref::<SwapError>().cloned());
            let want = Some(SwapError::QuoteZero { amount_in });
            assert_eq!(got, want, "{label}: got {got:?}, want {want:?}");
        }
    }

    /// An order id's `Display` (its hex form) is the 32 lowercase hex
    /// digits of its bytes, most significant nibble first.
    #[test]
    fn order_id_hex_is_lowercase_bytes_in_order() {
        let id = OrderId([
            0x00, 0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0x10, 0x32, 0x54, 0x76, 0x98,
            0xba, 0xff,
        ]);
        let want = "000123456789abcdef1032547698baff";
        assert_eq!(id.to_string(), want, "display: got {id}, want {want}");
    }
}
