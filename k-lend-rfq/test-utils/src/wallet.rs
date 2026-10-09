use anyhow::{anyhow, Result};
use solana_address::Address;
use solana_signer::Signer;
use zolana_client::Rpc;
use zolana_keypair::{ShieldedKeypair, SigningKey};
use zolana_program_test::fixture;
use zolana_test_utils::wallet::{sync_wallet, Wallet};
use zolana_transaction::{verify_spendable, AssetRegistry, DecryptionResult};

use k_lend_market_maker::Holdings;
use k_lend_rfq_sdk::pair::Pair;

const PAGE_LIMIT: u32 = 1_000;

pub struct TestWallet {
    pub wallet: Wallet,
    pub keypair: ShieldedKeypair,
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
        Ok(Self { wallet, keypair })
    }

    pub fn address(&self) -> Address {
        self.keypair.pubkey()
    }

    pub fn sync(&mut self, indexer: &(impl Rpc + Sync)) -> Result<()> {
        sync_wallet(&mut self.wallet, &self.keypair, indexer)
            .map_err(|e| anyhow!("sync wallet {}: {e:?}", self.keypair.pubkey()))?;
        self.sync_every_event(indexer)
    }

    /// `sync_wallet` at `v0.4.0-alpha` keeps one event per transaction
    /// signature, and an RFQ swap is one transaction with two transfers, the
    /// user's and the market maker's. Decrypt every event that matches this
    /// wallet's tags and add the UTXOs and nullifiers the sync dropped.
    /// Proofless deposits need no second pass: the sync keys them by leaf.
    fn sync_every_event(&mut self, indexer: &impl Rpc) -> Result<()> {
        let tags = vec![
            self.keypair.shielded_address()?.confidential_view_tag()?,
            self.keypair.recipient_bootstrap_view_tag(),
        ];
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
        decrypted.extend(&self.keypair, &transactions, &self.wallet.registry)?;
        let spendable = verify_spendable(&self.keypair, &decrypted)?;
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

    pub fn holdings(&self, pair: &Pair) -> Result<Holdings> {
        Ok(Holdings {
            collateral: self.balance(pair.token_mint, None)?.amount,
            shares: self.balance(pair.shares_mint, None)?.amount,
        })
    }
}
