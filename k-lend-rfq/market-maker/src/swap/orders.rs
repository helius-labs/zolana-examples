//! The maker's book of open orders. An order exists from its quote until
//! its first fill attempt or its expiry, and is removed before it is checked,
//! so no order id can be filled twice by the maker.

use std::time::Instant;

use dashmap::DashMap;
use k_lend_rfq_sdk::{
    pair::Pair,
    swap::{OrderId, Quote, SwapError},
};

/// An order the maker issued in `Inner::quote` and has not yet consumed.
/// The amounts a fill pays come from `quote`, never from the user's request.
#[derive(Debug, Clone, Copy)]
pub struct OpenOrder {
    /// The pair the order was quoted on; a fill must name the same pair.
    pub pair: Pair,
    /// The amounts and direction the maker committed to.
    pub quote: Quote,
    /// The user transfer width the offer advertised; the fill enforces this
    /// value, not the maker's current maximum.
    pub max_user_inputs: usize,
    /// Issue instant plus the `order_ttl` in force at quote time. The order is
    /// fillable while `Instant::now() < expires_at`.
    pub expires_at: Instant,
}

/// The orders the maker has issued and not yet consumed, keyed by order id.
///
/// Every order lives here from its quote until its first fill attempt or its
/// expiry. Expired orders are dropped by `sweep`, which runs on every quote, so
/// the map stays bounded by the quotes issued within one order TTL without a
/// background task.
#[derive(Debug, Default)]
pub struct OpenOrders {
    orders: DashMap<OrderId, OpenOrder>,
}

impl OpenOrders {
    /// Opens `order` under `id`. Ids are 128 random bits, so a collision
    /// that would overwrite an open order is not handled.
    pub fn insert(&self, id: OrderId, order: OpenOrder) {
        self.orders.insert(id, order);
    }

    /// Removes the order `id` and returns it if it may be filled for `pair`.
    ///
    /// Checks, in order:
    /// - an order with `id` is open, else `SwapError::UnknownOrder`;
    /// - `Instant::now() < order.expires_at`, else `SwapError::OrderExpired`;
    /// - `order.pair == *pair`, else `SwapError::OrderPairMismatch`.
    ///
    /// The order is consumed on the first fill attempt whatever the outcome:
    /// it is removed before any check runs, so an expired or mismatched order
    /// is gone as well, and a fill that fails later (invalid user transfer,
    /// insufficient inventory) does not reinstate it. The user asks for a new
    /// quote in every such case. A second fill of the same id therefore fails
    /// with `UnknownOrder`.
    pub fn take(&self, id: OrderId, pair: &Pair) -> Result<OpenOrder, SwapError> {
        let (_, order) = self
            .orders
            .remove(&id)
            .ok_or(SwapError::UnknownOrder { order: id })?;
        if Instant::now() >= order.expires_at {
            return Err(SwapError::OrderExpired { order: id });
        }
        if order.pair != *pair {
            return Err(SwapError::OrderPairMismatch { order: id });
        }
        Ok(order)
    }

    /// Drops every order whose `expires_at` has passed.
    pub fn sweep(&self) {
        let now = Instant::now();
        self.orders.retain(|_, order| now < order.expires_at);
    }

    /// Number of open orders that have not expired yet.
    pub fn unexpired(&self) -> usize {
        let now = Instant::now();
        self.orders
            .iter()
            .filter(|order| now < order.expires_at)
            .count()
    }
}

#[cfg(test)]
mod tests {
    //! Tested invariants:
    //! 1. `take` of an order quoted on one pair, named with another pair,
    //!    fails with `OrderPairMismatch` and consumes the order, so a retry on
    //!    the right pair fails with `UnknownOrder`.
    //! 2. `take` of an expired order fails with `OrderExpired` and consumes
    //!    it; `take` of an open order on its own pair returns it.

    use std::time::Duration;

    use k_lend_rfq_sdk::swap::Direction;
    use solana_address::Address;

    use super::*;

    const COLLATERAL_MINT: Address = Address::new_from_array([1; 32]);
    const QUOTE: Quote = Quote {
        direction: Direction::Deposit,
        amount_in: 100,
        amount_out: 99,
    };
    const ORDER_TTL: Duration = Duration::from_secs(60);

    /// Invariant 1: a fill naming a second pair the maker serves is refused
    /// with `OrderPairMismatch`, and the order is gone afterwards.
    #[test]
    fn take_rejects_order_of_another_pair() {
        let quoted = Pair::new(Address::new_from_array([10; 32]), COLLATERAL_MINT);
        let other = Pair::new(Address::new_from_array([11; 32]), COLLATERAL_MINT);
        let orders = OpenOrders::default();
        let id = OrderId([3; 16]);
        orders.insert(id, open_order(quoted, ORDER_TTL));

        let mismatched = orders.take(id, &other);
        assert!(
            matches!(mismatched, Err(SwapError::OrderPairMismatch { order }) if order == id),
            "got {mismatched:?}, want OrderPairMismatch of {id}"
        );
        let retried = orders.take(id, &quoted);
        assert!(
            matches!(retried, Err(SwapError::UnknownOrder { order }) if order == id),
            "got {retried:?}, want UnknownOrder of {id}"
        );
    }

    /// Invariant 2: an order past `expires_at` is refused with
    /// `OrderExpired` and consumed; an open one is returned with its quote.
    #[test]
    fn take_rejects_expired_and_returns_open_order() {
        let pair = Pair::new(Address::new_from_array([10; 32]), COLLATERAL_MINT);
        let orders = OpenOrders::default();
        let expired = OrderId([4; 16]);
        orders.insert(expired, open_order(pair, Duration::ZERO));
        let open = OrderId([5; 16]);
        orders.insert(open, open_order(pair, ORDER_TTL));

        let refused = orders.take(expired, &pair);
        assert!(
            matches!(refused, Err(SwapError::OrderExpired { order }) if order == expired),
            "got {refused:?}, want OrderExpired of {expired}"
        );
        assert_eq!(orders.unexpired(), 1, "open orders after the expired take");
        let taken = orders.take(open, &pair).map(|order| order.quote);
        assert_eq!(taken, Ok(QUOTE), "quote of the open order");
        assert_eq!(orders.unexpired(), 0, "open orders after both takes");
    }

    // Test fixtures and shared helpers.

    /// An order of `QUOTE` on `pair` expiring `ttl` from now.
    fn open_order(pair: Pair, ttl: Duration) -> OpenOrder {
        OpenOrder {
            pair,
            quote: QUOTE,
            max_user_inputs: 1,
            expires_at: Instant::now() + ttl,
        }
    }
}
