use anyhow::{anyhow, bail, Result};
use rust_client_example::{create_private_wallet, deposit_sol, deposit_spl, fund_key, register_asset};
use solana_address::Address;
use solana_keypair::{read_keypair_file, Keypair};
use solana_pubkey::Pubkey;
use solana_signature::Signature;
use solana_signer::Signer;
use zolana_client::{
    create_associated_token_account, create_withdrawal_sync, prover::transact::assemble,
    sync_wallet, CreateWithdrawal, InputCommitment, ProofCompressed, ProverClient, ProverInputs,
    Rpc, SignedTransaction, SolanaRpc, SpendProof, ZolanaIndexer,
};
use zolana_interface::{
    instruction::{Transact, TransactWithdrawal},
    SHIELDED_POOL_PROGRAM_ID,
};

fn main() -> Result<()> {
    // Connect to the devnet deployment.
    let indexer = ZolanaIndexer::new("http://202.8.10.77:8784/");
    let rpc_url = format!(
        "https://devnet.helius-rpc.com/?api-key={}",
        std::env::var("API_KEY").expect("set API_KEY"),
    );
    let mut rpc = SolanaRpc::new(rpc_url);
    let prover = ProverClient::new("http://202.8.10.77:3011".to_string());
    let payer_path = std::env::var("ZOLANA_PAYER_KEYPAIR")
        .unwrap_or_else(|_| format!("{}/.config/solana/id.json", std::env::var("HOME").unwrap_or_default()));
    let payer = read_keypair_file(&payer_path).map_err(|e| anyhow!("load payer {payer_path}: {e}"))?;
    let tree: Pubkey = std::env::var("ZOLANA_TREE").expect("set ZOLANA_TREE").parse()?;
    rpc.assert_executable(&Pubkey::new_from_array(SHIELDED_POOL_PROGRAM_ID))?;

    // Withdraw an SPL value. For a SOL withdrawal, skip register_asset, build the
    // wallet from `AssetRegistry::default()`, use `SOL_MINT` as the asset, and pass
    // the recipient's plain Solana pubkey (no token account needed).
    let (asset, registry) = register_asset(&mut rpc, &payer)?;
    let asset_address = Address::new_from_array(asset.mint.to_bytes());
    let (keypair, _funding, mut wallet) = create_private_wallet(&mut rpc, &payer, registry)?;

    // Deposit an SPL asset to withdraw and SOL for the transaction fee
    deposit_spl(&rpc, &payer, tree, &indexer, &keypair, &mut wallet, &asset, 10_000)?;
    deposit_sol(&rpc, &payer, tree, &indexer, &keypair, &mut wallet, 5_000_000)?;

    // A withdrawal exits to a public account. Create the recipient's token account
    // so the withdrawal can land the tokens; the SDK derives this same account from
    // the recipient pubkey and mint.
    let recipient = Keypair::new();
    fund_key(&mut rpc, &payer, &recipient.pubkey(), 1_000_000)?;
    let (_ata_sig, ata) =
        create_associated_token_account(&rpc, &payer, &recipient.pubkey(), &asset.mint)?;

    // Sync the wallet to see the current balance before spending it
    sync_wallet(&mut wallet, &indexer)?;

    // Build and sign the withdrawal. `create_withdrawal_sync` picks the input notes,
    // derives the recipient's token account from the mint, builds the transaction,
    // and signs it.
    let payer_address = Address::new_from_array(payer.pubkey().to_bytes());
    let withdrawal = create_withdrawal_sync(CreateWithdrawal {
        wallet: &wallet,
        authority: &keypair,
        owner_pubkey: Pubkey::default(),
        payer: payer_address,
        recipient: recipient.pubkey(),
        asset: asset_address,
        amount: 4_000,
    })?;

    // Prove and submit the signed withdrawal.
    let signature = submit_private_transaction(
        &rpc,
        &indexer,
        &prover,
        &payer,
        tree,
        withdrawal.signed,
        Some(withdrawal.withdrawal),
    )?;

    // Sync the private balance.
    sync_wallet(&mut wallet, &indexer)?;

    println!("ok withdrawal signature={signature} recipient_token_account={ata}");
    Ok(())
}

/// Prove and submit a signed private transaction. This is the sequence the SDK
/// runs for you: fetch the input proofs, assemble the witness, prove, then send
/// the `Transact` instruction. `withdrawal` names the public destination for a
/// withdrawal, or `None` for a pure shielded transfer.
fn submit_private_transaction(
    rpc: &SolanaRpc,
    indexer: &ZolanaIndexer,
    prover: &ProverClient,
    payer: &Keypair,
    tree: Pubkey,
    signed: SignedTransaction,
    withdrawal: Option<TransactWithdrawal>,
) -> Result<Signature> {
    let commitments = signed.input_commitments()?;
    let proofs = spend_proofs(indexer, tree, &commitments)?;
    // `assemble` builds the witness once: the nullifiers, root indices, and dummy
    // padding come from it, so the instruction data and the proof commit to the
    // same values.
    let assembled = assemble(signed, &proofs)?;
    let proof = match &assembled.prover_inputs {
        ProverInputs::P256(inputs) => prover.prove_transfer_p256(inputs)?,
        ProverInputs::Eddsa(inputs) => prover.prove_transfer(inputs)?,
    };
    let proof = ProofCompressed::try_from(proof)?.to_transact_proof();
    let data = assembled.with_proof(proof);
    let ix = Transact {
        payer: payer.pubkey(),
        tree,
        withdrawal,
        data,
    }
    .instruction();
    let instructions = [
        solana_compute_budget_interface::ComputeBudgetInstruction::set_compute_unit_limit(
            1_400_000,
        ),
        ix,
    ];
    let signature = rpc.create_and_send_transaction(
        &instructions,
        Address::new_from_array(payer.pubkey().to_bytes()),
        &[payer],
    )?;
    Ok(signature)
}

/// Fetch a spend proof for each input: an inclusion proof that the note is in the
/// tree, and a non-inclusion proof that its nullifier has not been spent.
fn spend_proofs(
    indexer: &ZolanaIndexer,
    tree: Pubkey,
    commitments: &[InputCommitment],
) -> Result<Vec<SpendProof>> {
    let tree_address = Address::new_from_array(tree.to_bytes());
    let leaves = commitments.iter().map(|c| c.utxo_hash).collect::<Vec<_>>();
    let nullifiers = commitments.iter().map(|c| c.nullifier).collect::<Vec<_>>();
    let state_proofs = indexer.get_merkle_proofs(tree_address, leaves)?.proofs;
    let nullifier_proofs = indexer
        .get_non_inclusion_proofs(tree_address, nullifiers)?
        .proofs;
    if state_proofs.len() != commitments.len() || nullifier_proofs.len() != commitments.len() {
        bail!("indexer returned incomplete input proofs");
    }
    Ok(state_proofs
        .into_iter()
        .zip(nullifier_proofs)
        .map(|(state, nullifier)| SpendProof { state, nullifier })
        .collect())
}
