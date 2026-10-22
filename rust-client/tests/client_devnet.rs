use std::{env, path::PathBuf};

use anyhow::{anyhow, ensure, Result};
use rust_client_example::{SetupContext, INDEXER_URL, PROVER_URL};
use solana_keypair::{read_keypair_file, Keypair};
use solana_signer::Signer;
use solana_system_interface::instruction::transfer;
use zolana_client::{
    ComputeBudgetConfig, ProofDataSource, ProverClient, Rpc, SolanaRpc, ZolanaClient,
};
use zolana_interface::{pda, PROGRAM_ID_PUBKEY};
use zolana_user_registry_interface::user_registry_program_id;

mod shared;

const FUNDING_LAMPORTS: u64 = 100_000_000;
const DEVNET_GENESIS_HASH: [u8; 32] = [
    206, 89, 219, 80, 128, 252, 44, 109, 59, 207, 124, 169, 7, 18, 211, 194, 229, 230, 194, 143,
    39, 240, 223, 187, 153, 83, 189, 176, 137, 76, 3, 171,
];

fn with_funded_payer(
    client: &ZolanaClient<SolanaRpc>,
    sponsor: &Keypair,
    run: impl FnOnce(&Keypair) -> Result<()>,
) -> Result<()> {
    let payer = Keypair::new();
    client.create_and_send_transaction(
        &[transfer(
            &sponsor.pubkey(),
            &payer.pubkey(),
            FUNDING_LAMPORTS,
        )],
        sponsor.pubkey(),
        &[sponsor],
        ComputeBudgetConfig::new(10_000),
    )?;
    let result = run(&payer);
    let refund = (|| -> Result<()> {
        let remaining = client.get_balance(payer.pubkey())?.saturating_sub(20_000);
        if remaining > 0 {
            client.create_and_send_transaction(
                &[transfer(&payer.pubkey(), &sponsor.pubkey(), remaining)],
                payer.pubkey(),
                &[&payer],
                ComputeBudgetConfig::new(10_000),
            )?;
        }
        Ok(())
    })();
    result?;
    refund
}

#[test]
fn rust_and_typescript_clients_on_devnet() -> Result<()> {
    let context = SetupContext {
        rpc_url: env::var("DEVNET_RPC_URL")?,
        indexer_url: INDEXER_URL.to_string(),
        prover_url: PROVER_URL.to_string(),
        tree: pda::tree(0),
    };
    let rpc = SolanaRpc::new(&context.rpc_url);
    // Devnet genesis: EtWTRABZaYq6iMfeYKouRu166VU2xqa1wcaWoxPkrZBG.
    ensure!(
        rpc.genesis_hash()? == DEVNET_GENESIS_HASH,
        "DEVNET_RPC_URL must point to Solana devnet"
    );
    rpc.assert_executable(&PROGRAM_ID_PUBKEY)?;
    rpc.assert_executable(&user_registry_program_id())?;
    let prover = ProverClient::new(context.prover_url.clone());
    ensure!(prover.has_tee(), "devnet proving must use TEE attestation");
    prover.attest()?;
    println!("Devnet Photon: {INDEXER_URL}; attested TEE prover: {PROVER_URL}");
    let client = ZolanaClient::from_urls(rpc, &context.indexer_url, &context.prover_url)?
        .with_proof_data_source(ProofDataSource::Client);
    let keypair_path = env::var("DEVNET_PAYER_KEYPAIR_PATH")?;
    let sponsor = read_keypair_file(keypair_path)
        .map_err(|error| anyhow!("load devnet test sponsor: {error}"))?;
    ensure!(
        client.get_balance(sponsor.pubkey())? >= FUNDING_LAMPORTS * 2,
        "fund the devnet CI wallet with at least 0.2 SOL"
    );
    with_funded_payer(&client, &sponsor, |payer| {
        shared::run_rust_examples(&client, context.tree, payer)
    })?;
    let payer_path = PathBuf::from(env::var("DEVNET_PAYER_KEYPAIR_PATH")?)
        .with_file_name("typescript-devnet-payer.json");
    let result = with_funded_payer(&client, &sponsor, |payer| {
        shared::run_typescript_examples(&context, payer, &payer_path)
    });
    std::fs::remove_file(payer_path).ok();
    result
}
