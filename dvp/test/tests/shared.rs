use std::{
    env,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{anyhow, Result};
use dvp_program::error::DvpError;
use dvp_sdk::{
    encryption::TERMS_RECIPIENTS,
    instructions::{
        create_dvp::{CreateDvp, CreateDvpDerivation, CreateDvpTransactionParams},
        fund_dvp::{FundDvp, FundDvpParams},
    },
    state::{Leg, SwapDvp},
};
use solana_address::Address;
use solana_instruction::Instruction;
use solana_keypair::Keypair;
use solana_signature::Signature;
use solana_signer::Signer;
use zolana_client::{ComputeBudgetConfig, ProverClient, Rpc};
use zolana_interface::pda::spl_token_program_id;
use zolana_keypair::{ShieldedAddress, ShieldedKeypair};
use zolana_program_test::{
    fixture,
    localnet::{FixtureLocalnet, LocalnetPaths, LocalnetPorts},
};
use zolana_test_utils::{
    test_validator_asserts::wait_for_non_inclusion_proof,
    wallet::{Deposit as ShieldDeposit, DepositParams},
};
use zolana_transaction::{decrypt_spendable, AssetRegistry, WalletUtxo, SOL_MINT};

/// The whole per-transaction budget: an instruction verifies its own proof
/// and the SPP proof.
pub const TRANSACT_CU_LIMIT: u32 = 1_400_000;
/// How long a party polls the indexer for a transaction it expects.
pub const INDEX_TIMEOUT: Duration = Duration::from_secs(60);

pub const DELIVERY_AMOUNT: u64 = 400_000_000;
pub const PAYMENT_AMOUNT: u64 = 250_000_000;
/// Seconds from setup until the test DvPs expire.
const EXPIRY_AFTER: i64 = 3_600;

/// One party: a funded Solana keypair and the shielded keypair derived from
/// it, whose viewing key reads the party's terms messages.
pub struct Actor {
    pub solana: Keypair,
    pub shielded: ShieldedKeypair,
}

impl Actor {
    fn new(index: u8) -> Result<Self> {
        let solana = fixture::actor(index);
        let shielded = ShieldedKeypair::from_keypair(&solana)?;
        Ok(Self { solana, shielded })
    }

    pub fn address(&self) -> Result<ShieldedAddress> {
        Ok(self.shielded.shielded_address()?)
    }
}

pub struct TestEnv {
    /// The localnet with its client and default tree. Dropping it stops the
    /// validator, so it lives as long as the test.
    pub localnet: FixtureLocalnet,
    /// Fee payer and SPP payer of every transaction.
    pub payer: Keypair,
    /// Also the creator of the test DvPs.
    pub seller: Actor,
    pub buyer: Actor,
    pub settlement_authority: Actor,
}

/// `file` under this workspace's `target/deploy`, where `cargo build-sbf`
/// writes the program.
fn deploy_path(file: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../target/deploy")
        .join(file)
}

/// `ZOLANA_PROVER_KEYS_DIR`, else `~/.config/zolana/proving-keys`, the
/// directory `zolana test-env` keeps the proving keys in.
fn proving_keys_dir() -> Result<PathBuf> {
    if let Ok(dir) = env::var("ZOLANA_PROVER_KEYS_DIR") {
        return Ok(dir.into());
    }
    let home = env::var("HOME").map_err(|_| anyhow!("HOME is not set"))?;
    Ok(Path::new(&home).join(".config/zolana/proving-keys"))
}

/// The CLI, prover, Photon, surfpool and pool program come from the
/// environment (`ZOLANA_CLI_BIN`, `PROVER_BIN`, `ZOLANA_PHOTON_BIN`,
/// `SURFPOOL_BIN`, `SHIELDED_POOL_PROGRAM_PATH`). The keys and the scratch
/// directory are set here: `LocalnetPaths::workspace` alone resolves them
/// inside the cargo git checkout of zolana.
fn localnet_paths() -> Result<LocalnetPaths> {
    Ok(LocalnetPaths {
        proving_keys_dir: proving_keys_dir()?,
        scratch_dir: Path::new(env!("CARGO_MANIFEST_DIR")).join("../target/localnet"),
        ..LocalnetPaths::workspace()
    })
}

/// Boot the localnet of test number `test` ([`LocalnetPorts::for_test`]);
/// tests running in parallel take distinct numbers.
pub fn setup(test: u16) -> Result<TestEnv> {
    let localnet = FixtureLocalnet::start(
        "zolana-dvp",
        LocalnetPorts::for_test(test)?,
        vec![(dvp_program::ID, deploy_path("dvp_program.so"))],
        &localnet_paths()?,
    )?;
    Ok(TestEnv {
        localnet,
        payer: fixture::payer(),
        seller: Actor::new(0)?,
        buyer: Actor::new(1)?,
        settlement_authority: Actor::new(2)?,
    })
}

impl TestEnv {
    /// The terms recipients in message order.
    pub fn parties(&self) -> [&Actor; TERMS_RECIPIENTS] {
        [&self.seller, &self.buyer, &self.settlement_authority]
    }

    /// Terms the seller creates: the fixture SPL mint delivered against
    /// SOL, settled to and refunded to the parties' own shielded addresses,
    /// expiring an hour from now. `nonce` tells DvPs apart.
    pub fn terms(&self, nonce: u64) -> Result<SwapDvp> {
        let seller = self.seller.address()?;
        let buyer = self.buyer.address()?;
        let now = i64::try_from(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs())?;
        Ok(SwapDvp {
            seller: self.seller.solana.pubkey(),
            buyer: self.buyer.solana.pubkey(),
            delivery_mint: fixture::spl_mint(),
            payment_mint: SOL_MINT,
            settlement_authority: self.settlement_authority.solana.pubkey(),
            delivery_amount: DELIVERY_AMOUNT,
            payment_amount: PAYMENT_AMOUNT,
            expiry_timestamp: now + EXPIRY_AFTER,
            nonce,
            ref_string: [0; 64],
            seller_settlement_destination: seller,
            buyer_settlement_destination: buyer,
            earliest_settlement_timestamp: None,
            seller_refund_destination: seller,
            buyer_refund_destination: buyer,
            creator: self.seller.solana.pubkey(),
            blinding: zolana_keypair::random_blinding(),
        })
    }

    /// The create instruction of `terms`, with both proofs: the SPP proof
    /// from the localnet prover against a fresh non-inclusion proof of the
    /// DvP address, and the create proof from the local gnark prover.
    pub fn prove_create(&self, terms: &SwapDvp) -> Result<CreateDvp> {
        let tree_id = self.localnet.tree_id;
        let address = CreateDvpDerivation::new(terms, tree_id)?.address;
        let non_inclusion =
            wait_for_non_inclusion_proof(&self.localnet.client, self.localnet.tree, address);
        let [seller, buyer, settlement_authority] = self.parties();
        CreateDvpTransactionParams {
            terms: terms.clone(),
            payer: self.payer.pubkey(),
            recipients: [
                seller.address()?,
                buyer.address()?,
                settlement_authority.address()?,
            ],
            tree_id,
            non_inclusion,
        }
        .build()?
        .prove(&ProverClient::local(), self.localnet.tree)
    }

    /// Submits `instruction` as a transaction **v1** message, whose 4096-byte
    /// limit holds a proof-carrying DvP instruction, paid by the payer and
    /// signed by `signers` too. The compute ceilings live in the message
    /// header. A priority fee makes an otherwise identical message distinct,
    /// which the replay negatives rely on.
    pub fn send(
        &self,
        instruction: Instruction,
        signers: &[&dyn Signer],
        priority_fee: Option<u64>,
    ) -> Result<Signature> {
        let budget = ComputeBudgetConfig::new(TRANSACT_CU_LIMIT);
        let budget = match priority_fee {
            Some(fee) => budget.with_priority_fee(fee),
            None => budget,
        };
        let mut all_signers: Vec<&dyn Signer> = vec![&self.payer];
        all_signers.extend_from_slice(signers);
        Ok(self.localnet.client.create_and_send_transaction(
            std::slice::from_ref(&instruction),
            self.payer.pubkey(),
            &all_signers,
            budget,
        )?)
    }

    /// The pool's assets: SOL and the fixture SPL mint.
    pub fn assets(&self) -> Result<AssetRegistry> {
        let mut assets = AssetRegistry::default();
        assets.insert(fixture::SPL_ASSET_ID, fixture::spl_mint())?;
        Ok(assets)
    }

    /// Shields `amount` of `mint` from the payer's public balance (its SOL
    /// or its fixture SPL token account) into `actor`'s private balance, and
    /// returns the note once the indexer serves it.
    pub fn shield(&self, actor: &Actor, mint: Address, amount: u64) -> Result<WalletUtxo> {
        let recipient = actor.address()?;
        let spl = mint != SOL_MINT;
        let deposit = ShieldDeposit::new(DepositParams {
            recipient: &recipient,
            asset: mint,
            amount,
            spl_token_account: spl.then(fixture::payer_token_account),
            spl_token_program: Some(spl_token_program_id()),
            memo: None,
        })?;
        let client = &self.localnet.client;
        let signature = deposit.send(client, &self.payer, self.localnet.tree, &self.payer)?;
        client.confirm_private_transaction_sync(signature)?;
        self.received_note(actor, deposit.view_tag(), mint, signature)
    }

    /// `actor`'s spendable note of `mint` that transaction `signature`
    /// created, found by `view_tag` and decrypted the way a wallet syncs.
    pub fn received_note(
        &self,
        actor: &Actor,
        view_tag: [u8; 32],
        mint: Address,
        signature: Signature,
    ) -> Result<WalletUtxo> {
        let transactions = self
            .localnet
            .client
            .get_shielded_transactions_by_tags(vec![view_tag], None, None, None)?
            .transactions;
        decrypt_spendable(&actor.shielded, &transactions, &self.assets()?)
            .map_err(|e| anyhow!("decrypt shielded notes: {e:?}"))?
            .balances
            .get_balance(mint)
            .into_iter()
            .flat_map(|balance| balance.utxos.iter())
            .find(|note| note.tx_signature == signature)
            .cloned()
            .ok_or_else(|| anyhow!("shielded note of {signature} not found"))
    }

    /// `funder` sends `amount` of `leg`'s mint from `note` to the DvP's
    /// deposit address in a plain SPP transfer it pays and signs, and waits
    /// until the indexer serves it.
    pub fn fund(
        &self,
        funder: &Actor,
        terms: &SwapDvp,
        leg: Leg,
        amount: u64,
        note: WalletUtxo,
    ) -> Result<Signature> {
        let client = &self.localnet.client;
        let proof_inputs = FundDvpParams {
            terms: terms.clone(),
            leg,
            amount,
            authority_viewing_pubkey: self.settlement_authority.shielded.viewing_pubkey(),
            inputs: vec![note],
            payer: funder.solana.pubkey(),
            tree_id: self.localnet.tree_id,
        }
        .build(&funder.shielded)?;
        let spp = client.prove_transact(proof_inputs, None, &funder.shielded)?;
        let instruction = FundDvp {
            payer: funder.solana.pubkey(),
            tree: self.localnet.tree,
            spp,
        }
        .instruction();
        let signature = client.create_and_send_transaction(
            &[instruction],
            funder.solana.pubkey(),
            &[&funder.solana],
            ComputeBudgetConfig::new(TRANSACT_CU_LIMIT),
        )?;
        client.confirm_private_transaction_sync(signature)?;
        Ok(signature)
    }
}

// A custom program error surfaces as `Custom(<decimal>)` or as the
// program-log line `custom program error: 0x<hex>`; match only those
// delimited forms.
pub fn assert_dvp_error(context: &str, result: Result<Signature>, error: DvpError) {
    let code = error as u32;
    let text = match result {
        Ok(signature) => panic!("{context}: unexpectedly succeeded ({signature})"),
        Err(err) => format!("{err:?}"),
    };
    assert!(
        text.contains(&format!("Custom({code})")) || text.contains(&format!("0x{code:x}")),
        "{context}: expected {error:?} ({code}) in: {text}"
    );
}
