use std::sync::Arc;

use anyhow::{anyhow, Result};
use solana_address::Address;
use solana_signer::Signer;
use zolana_client::{ProofAuthority, Rpc};
use zolana_keypair::{ShieldedAddress, ShieldedKeypair, SigningKey};
use zolana_program_test::{fixture, localnet::FixtureLocalnet};
use zolana_test_utils::wallet::{sync_wallet, Wallet};
use zolana_transaction::{
    verify_spendable, AssetRegistry, DecryptionResult, ShieldedKeys, WalletUtxo,
};

use k_lend_market_maker::{Holdings, IdentityConfig};
use k_lend_rfq_sdk::{
    pair::Pair,
    transfer::{Transfer, TransferInstruction},
};

use crate::chain::blocking;

const PAGE_LIMIT: u32 = 1_000;

pub struct TestWallet {
    pub wallet: Wallet,
    keypair: Arc<ShieldedKeypair>,
}

impl std::ops::Deref for TestWallet {
    type Target = Wallet;
    fn deref(&self) -> &Self::Target {
        &self.wallet
    }
}

impl TestWallet {
    pub fn new(actor: u8, assets: &AssetRegistry) -> Result<Self> {
        let solana = fixture::actor(actor);
        let seed: [u8; 32] = solana
            .to_bytes()
            .get(..32)
            .ok_or_else(|| anyhow!("ed25519 keypair without a seed"))?
            .try_into()?;
        let keypair = ShieldedKeypair::from_keypair(SigningKey::from_ed25519_bytes(&seed))?;
        let wallet = Wallet::new(keypair.shielded_address()?, assets.clone())
            .map_err(|e| anyhow!("wallet of actor {actor}: {e:?}"))?;
        Ok(Self {
            wallet,
            keypair: Arc::new(keypair),
        })
    }

    pub fn keys(&self) -> &dyn ShieldedKeys {
        self.keypair.as_ref()
    }

    pub fn authority(&self) -> &dyn ProofAuthority {
        self.keypair.as_ref()
    }

    pub fn signer(&self) -> &dyn Signer {
        self.keypair.as_ref()
    }

    /// The market maker's identity backed by this wallet's keypair.
    pub fn identity_config(&self) -> IdentityConfig {
        IdentityConfig::from_keypair(self.keypair.clone())
    }

    pub fn address(&self) -> Address {
        self.keypair.pubkey()
    }

    pub fn sync(&mut self, indexer: &(impl Rpc + Sync)) -> Result<()> {
        sync_wallet(&mut self.wallet, self.keypair.as_ref(), indexer)
            .map_err(|e| anyhow!("sync wallet {}: {e:?}", self.keypair.pubkey()))?;
        self.sync_every_event(indexer)
    }

    /// `sync_wallet` at `v0.4.0-alpha` keeps one event per transaction
    /// signature, and an RFQ swap is one transaction with two transfers, the
    /// user's and the market maker's. Decrypt every event that matches this
    /// wallet's tags and add the UTXOs and nullifiers the sync dropped.
    /// Proofless deposits need no second pass: the sync keys them by leaf.
    fn sync_every_event(&mut self, indexer: &impl Rpc) -> Result<()> {
        let address = self.keys().address()?;
        let tags = vec![address.confidential_view_tag()?, address.viewing_pubkey.x()];
        let mut transactions = Vec::new();
        let mut cursor = None;
        loop {
            let response = indexer.get_shielded_transactions_by_tags(
                tags.clone(),
                cursor,
                Some(PAGE_LIMIT),
                None,
            )?;
            transactions.extend(
                response
                    .transactions
                    .into_iter()
                    .filter(|tx| !tx.proofless && tx.tx_viewing_pk.is_some() && tx.salt.is_some()),
            );
            cursor = response.next_cursor;
            if cursor.is_none() {
                break;
            }
        }

        let mut decrypted = DecryptionResult::default();
        decrypted.extend(self.keys(), &transactions, &self.wallet.registry)?;
        let spendable = verify_spendable(self.keys(), &decrypted)?;
        for utxo in spendable
            .balances
            .assets
            .into_iter()
            .flat_map(|balance| balance.utxos)
        {
            if !self
                .wallet
                .utxos
                .iter()
                .any(|known| known.utxo_hash == utxo.utxo_hash)
            {
                self.wallet.utxos.push(utxo);
            }
        }
        self.wallet.nullifiers.extend(decrypted.spent_nullifiers);
        Ok(())
    }

    /// The first UTXO of `asset` this wallet holds.
    pub fn first_utxo(&self, asset: Address) -> Result<WalletUtxo> {
        self.balance(asset, None)?
            .utxos
            .first()
            .cloned()
            .ok_or_else(|| anyhow!("wallet {} holds no utxo of {asset}", self.address()))
    }

    /// Proves, with this wallet's keys and outside the market maker, a
    /// transfer of `amount` from `inputs` to `recipient` naming `payer` as fee
    /// payer, as wide as `inputs`, on `localnet`'s tree.
    pub fn transfer(
        &self,
        localnet: &FixtureLocalnet,
        inputs: Vec<WalletUtxo>,
        amount: u64,
        recipient: ShieldedAddress,
        payer: Address,
    ) -> Result<TransferInstruction> {
        blocking(|| {
            Transfer {
                width: inputs.len(),
                inputs,
                amount,
                recipient,
                payer,
                tree: localnet.tree,
                tree_id: localnet.tree_id,
            }
            .prove(&localnet.client, self.keys(), self.authority())
        })
    }

    pub fn holdings(&self, pair: &Pair) -> Result<Holdings> {
        Ok(Holdings {
            collateral: self.balance(pair.token_mint, None)?.amount,
            shares: self.balance(pair.shares_mint, None)?.amount,
        })
    }
}
