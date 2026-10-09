use anyhow::Result;
use k_lend_rfq_sdk::{
    pair::{Pair, VaultState},
    swap::{Offer, Order},
    transfer::Receiver,
    user::{select_inputs, QuoteCheck, UserOrder},
};
use solana_address::Address;
use solana_message::VersionedMessage;
use solana_signature::Signature;
use solana_signer::Signer;
use zolana_client::{SolanaRpc, ZolanaClient};
use zolana_keypair::ShieldedAddress;

use k_lend_market_maker::Holdings;

use crate::{chain::blocking, wallet::TestWallet};

pub struct User {
    wallet: TestWallet,
    fee_bps: u64,
    /// The tree the user's outputs go to, and its raw id.
    tree: Address,
    tree_id: u16,
}

impl User {
    pub fn new(wallet: TestWallet, fee_bps: u64, tree: Address, tree_id: u16) -> Self {
        Self {
            wallet,
            fee_bps,
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
            .prove(client, &self.wallet.keypair)
        })
    }

    pub async fn verify_quote(
        &self,
        client: &ZolanaClient<SolanaRpc>,
        pair: &Pair,
        order: &Order,
        message: &VersionedMessage,
    ) -> Result<()> {
        let rate = blocking(|| VaultState::read(client.rpc(), &pair.vault))?;
        QuoteCheck {
            order,
            message,
            pair,
            rate: &rate,
            fee_bps: self.fee_bps,
        }
        .verify(&Receiver {
            keypair: &self.wallet.keypair,
            registry: &self.wallet.registry,
            tree_id: self.tree_id,
        })
    }

    pub fn sign(&self, message: &VersionedMessage) -> Result<Signature> {
        Ok(self.wallet.keypair.try_sign_message(&message.serialize())?)
    }
}
