use anyhow::Result;
use k_lend_rfq_sdk::{
    pair::Pair,
    swap::{Offer, Order},
    transfer::Receiver,
    user::{select_inputs, QuoteCheck, UserOrder},
};
use solana_address::Address;
use solana_message::VersionedMessage;
use solana_signature::Signature;
use zolana_client::{SolanaRpc, ZolanaClient};
use zolana_keypair::ShieldedAddress;

use k_lend_market_maker::{Holdings, MakerFill, MarketMaker};

use crate::{
    chain::{blocking, confirm_indexed},
    wallet::TestWallet,
};

pub struct User {
    wallet: TestWallet,
    /// The tree the user's outputs go to, and its raw id.
    tree: Address,
    tree_id: u16,
}

impl User {
    pub fn new(wallet: TestWallet, tree: Address, tree_id: u16) -> Self {
        Self {
            wallet,
            tree,
            tree_id,
        }
    }

    pub fn wallet(&self) -> &TestWallet {
        &self.wallet
    }

    pub fn identity(&self) -> ShieldedAddress {
        self.wallet.identity
    }

    pub async fn sync(&mut self, client: &ZolanaClient<SolanaRpc>) -> Result<()> {
        blocking(|| self.wallet.sync(client.indexer()))
    }

    pub fn holdings(&self, pair: &Pair) -> Result<Holdings> {
        self.wallet.holdings(pair)
    }

    pub async fn order(
        &self,
        client: &ZolanaClient<SolanaRpc>,
        pair: &Pair,
        offer: &Offer,
    ) -> Result<Order> {
        self.order_with_width(client, pair, offer, None).await
    }

    pub async fn order_with_width(
        &self,
        client: &ZolanaClient<SolanaRpc>,
        pair: &Pair,
        offer: &Offer,
        width: Option<usize>,
    ) -> Result<Order> {
        let (asset_in, _) = offer.quote.direction.assets(pair);
        let inputs = select_inputs(
            self.wallet.balance(asset_in, None)?.utxos,
            asset_in,
            offer.quote.amount_in,
            offer.max_user_inputs,
        )?;
        blocking(|| {
            UserOrder {
                offer: *offer,
                inputs,
                width,
                tree: self.tree,
                tree_id: self.tree_id,
            }
            .prove(client, self.wallet.keys(), self.wallet.authority())
        })
    }

    pub fn verify_quote(
        &self,
        pair: &Pair,
        order: &Order,
        message: &VersionedMessage,
    ) -> Result<()> {
        QuoteCheck {
            order,
            message,
            pair,
        }
        .verify(&Receiver {
            keys: self.wallet.keys(),
            registry: &self.wallet.registry,
            tree_id: self.tree_id,
        })
    }

    pub fn sign(&self, message: &VersionedMessage) -> Result<Signature> {
        Ok(self
            .wallet
            .signer()
            .try_sign_message(&message.serialize())?)
    }

    /// Verifies `fill` against `order`, signs it and has `market_maker`
    /// settle it; returns the landed signature without waiting for the
    /// indexer.
    pub async fn settle(
        &self,
        market_maker: &MarketMaker,
        pair: &Pair,
        order: &Order,
        fill: &MakerFill,
    ) -> Result<Signature> {
        self.verify_quote(pair, order, &fill.fill.message)?;
        let user_signature = self.sign(&fill.fill.message)?;
        market_maker.settle(&fill.fill, user_signature).await
    }

    /// [`Self::settle`], then waits until the swap is indexed and syncs this
    /// user and `market_maker`.
    pub async fn complete(
        &mut self,
        client: &ZolanaClient<SolanaRpc>,
        market_maker: &MarketMaker,
        pair: &Pair,
        order: &Order,
        fill: &MakerFill,
    ) -> Result<()> {
        let signature = self.settle(market_maker, pair, order, fill).await?;
        confirm_indexed(client, signature, "swap")?;
        self.sync(client).await?;
        market_maker.sync().await
    }
}
