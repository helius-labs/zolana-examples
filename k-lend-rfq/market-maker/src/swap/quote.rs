//! Quoting: prices a request at the vault price minus the fee, checks the
//! maker can and wants to take it, and registers it as an open order. Every
//! amount a later fill pays comes from the order registered here.

use std::time::{Instant, SystemTime};

use anyhow::{anyhow, Result};
use solana_address::Address;

use k_lend_rfq_sdk::{
    pair::Pair,
    swap::{Direction, Offer, OrderId, Quote, SwapError},
    transfer::{smallest_shape, USER_OUTPUTS},
};

use crate::{
    api::Inner,
    inventory::balance::{
        pending::{range_check, Flow},
        select::width,
    },
    swap::orders::OpenOrder,
    transactions::{budget::MAKER_MIN_OUTPUTS, kvault::read_vault},
};

impl Inner {
    /// Prices `amount_in` of `direction` on `pair` and opens an order for it.
    ///
    /// Expired orders are swept first. Checks, in order, before the order
    /// opens:
    /// 1. the maker serves `pair` (configured, same PDAs, not retiring), else
    ///    `MakerError::PairNotServed`;
    /// 2. the vault reads and prices `amount_in` (`Quote::price`), else the
    ///    rpc error or the `VaultError` the program would fail with;
    /// 3. the priced `amount_out` is not zero, else `SwapError::QuoteZero`
    ///    from `Quote::price` (a zero payout would open an order that the
    ///    user pays for and the fill then refuses);
    /// 4. paying `amount_out` keeps the outgoing asset's net balance at or
    ///    above its range minimum, else `SwapError::OutsideTargetRange`;
    /// 5. receiving `amount_in` keeps the incoming asset's net balance at or
    ///    below its range maximum, else `SwapError::OutsideTargetRange`;
    /// 6. the available inventory can pay `amount_out` within
    ///    `max_maker_inputs` inputs and leaves room for at least one user
    ///    input (`Inner::quoted_user_inputs`), else
    ///    `SwapError::InsufficientInventory` or `SwapError::MakerTransferTooWide`.
    ///
    /// The order records the pair, the quote, the user transfer width put in
    /// the offer, and its expiry instant (`order_ttl` from the settings at
    /// quote time; later config updates do not move it). The offer carries the
    /// same expiry as unix seconds, and `marker_lamports`, the rent minimum
    /// the fill locks in the order's marker account (`order_marker_instruction`).
    /// `Inner::fill` takes the amounts from this record, never from the
    /// request.
    pub async fn quote(&self, pair: &Pair, direction: Direction, amount_in: u64) -> Result<Offer> {
        self.orders.sweep();
        let (fee_bps, order_ttl) = {
            let settings = self.settings();
            settings.serves(pair)?;
            (settings.quotes.fee_bps, settings.quotes.order_ttl)
        };
        let rate = read_vault(self.services.rpc.as_ref(), pair.vault).await?;
        let quote = Quote::price(&rate, direction, amount_in, fee_bps)?;
        let (asset_in, asset_out) = direction.assets(pair);
        self.check_range(asset_out, quote.amount_out, Flow::Out)?;
        self.check_range(asset_in, amount_in, Flow::In)?;
        let max_user_inputs = self.quoted_user_inputs(asset_out, quote.amount_out)?;
        let overflow = || anyhow!("order ttl {order_ttl:?} overflows the clock");
        let deadline = Instant::now().checked_add(order_ttl).ok_or_else(overflow)?;
        let expires_at = SystemTime::now()
            .checked_add(order_ttl)
            .ok_or_else(overflow)?
            .duration_since(SystemTime::UNIX_EPOCH)?
            .as_secs();
        let id = OrderId::random();
        self.orders.insert(
            id,
            OpenOrder {
                pair: *pair,
                quote,
                max_user_inputs,
                expires_at: deadline,
            },
        );
        Ok(Offer {
            id,
            expires_at,
            quote,
            maker: self.identity.own,
            fee_payer: self.identity.payer,
            max_user_inputs,
            user_outputs: USER_OUTPUTS,
            marker_lamports: self.marker_lamports,
        })
    }

    fn check_range(&self, asset: Address, amount: u64, flow: Flow) -> Result<(), SwapError> {
        range_check(
            asset,
            self.services.pending.net_balance(&asset),
            amount,
            flow,
            self.settings().range(&asset),
        )
    }

    /// The widest user transfer that fits next to the maker transfer a fill
    /// could build from the inventory available right now.
    ///
    /// Errors, in order: `SwapError::AmountOverflow` if the available amounts
    /// overflow; when no selection of at most `max_maker_inputs` UTXOs covers
    /// `amount`, `SwapError::InsufficientInventory` if their total is short,
    /// else `SwapError::MakerTransferTooWide`; `SwapError::NoSupportedShape`
    /// if no shape fits the maker inputs; `SwapError::MakerTransferTooWide`
    /// when no user input fits next to the maker transfer.
    ///
    /// The maker's inputs are sized from `reservations.available(asset)`, the
    /// same list `Inner::fill` passes to `select`. It excludes UTXOs reserved
    /// by an in-flight step (another fill, a consolidation or a rebalance may
    /// spend them) and UTXOs whose leaf index is not yet known (they cannot be
    /// proven). Counting either would let the quote assume a narrower maker
    /// transfer than the fill can build, leaving the user too little room in
    /// the shared transaction. The result is stored on the order, so the fill
    /// enforces the width the quote promised.
    fn quoted_user_inputs(&self, asset: Address, amount: u64) -> Result<usize> {
        let utxos: Vec<u64> = self
            .services
            .pending
            .reservations
            .available(&asset)
            .iter()
            .map(|utxo| utxo.amount())
            .collect();
        let available = utxos
            .iter()
            .try_fold(0u64, |total, amount| total.checked_add(*amount))
            .ok_or(SwapError::AmountOverflow {
                context: "maker inventory",
            })?;
        let max_inputs = self.max_maker_inputs;
        let too_wide = SwapError::MakerTransferTooWide {
            asset,
            required: amount,
            max_inputs,
        };
        let Some(inputs) = width(utxos, amount, max_inputs) else {
            if available < amount {
                return Err(SwapError::InsufficientInventory {
                    asset,
                    required: amount,
                    available,
                }
                .into());
            }
            return Err(too_wide.into());
        };
        let maker =
            smallest_shape(inputs, MAKER_MIN_OUTPUTS).ok_or(SwapError::NoSupportedShape {
                inputs,
                outputs: MAKER_MIN_OUTPUTS,
            })?;
        Ok(self
            .services
            .budget
            .max_user_inputs(maker)?
            .ok_or(too_wide)?)
    }
}
