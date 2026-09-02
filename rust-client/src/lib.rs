//! Shared environment settings for the instruction example: the fee payer,
//! Helius RPC, Photon indexer, and prover.

use anyhow::{anyhow, Result};
use solana_address::Address;
use solana_instruction::{AccountMeta, Instruction};
use solana_keypair::{read_keypair_file, Keypair};
use solana_pubkey::Pubkey;
use solana_signer::Signer;
use zolana_client::{Rpc, SolanaRpc};
use zolana_interface::{pda, state::SplAssetRegistry, DEFAULT_TREE_ADDRESS};
use zolana_keypair::{ShieldedAddress, ShieldedKeypair};
use zolana_wallet::create_associated_token_account;

/// The RPC, Photon indexer, and prover the examples talk to.
pub const RPC_URL: &str = "https://devnet.helius-rpc.com";
pub const INDEXER_URL: &str = "http://zolnet-devnet-1779374825.eu-north-1.elb.amazonaws.com";
pub const PROVER_URL: &str = "http://zolnet-devnet-1779374825.eu-north-1.elb.amazonaws.com:3001";
// localnet: pub const RPC_URL: &str = "http://127.0.0.1:8899";
// localnet: pub const INDEXER_URL: &str = "http://127.0.0.1:8784";
// localnet: pub const PROVER_URL: &str = "http://127.0.0.1:3001";

/// Enough public SOL for the 1 SOL deposit plus fees.
const SENDER_LAMPORTS: u64 = 2_000_000_000;

/// Service URLs, a funded sender, and a fresh recipient address.
pub struct SetupContext {
    pub rpc_url: String,
    pub indexer_url: String,
    pub prover_url: String,
    pub tree: Address,
    pub sender: ShieldedKeypair,
    pub recipient_address: ShieldedAddress,
}

/// Read the environment settings: the fee payer (`ZOLANA_PAYER_KEYPAIR`,
/// defaults to the Solana CLI wallet) and the `API_KEY` for the Helius devnet
/// RPC. Defaults are Helius plus the Photon/prover ALB. Toggle the
/// `localnet:` lines to run against a local stack instead.
pub fn setup() -> Result<SetupContext> {
    dotenvy::dotenv().ok();
    let payer_path = std::env::var("ZOLANA_PAYER_KEYPAIR")
        .unwrap_or_else(|_| "~/.config/solana/id.json".to_string());
    let payer_path = shellexpand::tilde(&payer_path).into_owned();
    let payer =
        read_keypair_file(&payer_path).map_err(|e| anyhow!("load payer {payer_path}: {e}"))?;
    let tree = DEFAULT_TREE_ADDRESS
        .parse()
        .map_err(|e| anyhow!("parse tree address: {e}"))?;

    let api_key = std::env::var("API_KEY").map_err(|_| anyhow!("set API_KEY"))?;
    let rpc_url = format!("{RPC_URL}/?api-key={api_key}");
    // localnet: let rpc_url = RPC_URL.to_string();

    let sender_solana = Keypair::new();
    let rpc = SolanaRpc::new(rpc_url.clone());
    let ix = solana_system_interface::instruction::transfer(
        &payer.pubkey(),
        &sender_solana.pubkey(),
        SENDER_LAMPORTS,
    );
    rpc.create_and_send_transaction(&[ix], payer.pubkey(), &[&payer])?;

    let sender = ShieldedKeypair::from_solana_keypair(&sender_solana)?;
    let recipient_address =
        ShieldedKeypair::from_solana_keypair(&Keypair::new())?.shielded_address()?;

    Ok(SetupContext {
        rpc_url,
        indexer_url: INDEXER_URL.to_string(),
        prover_url: PROVER_URL.to_string(),
        tree,
        sender,
        recipient_address,
    })
}

/// Enough raw token units for the 1_000_000_000 SPL deposit in the *_spl examples.
const SPL_DEPOSIT_AMOUNT: u64 = 1_000_000_000;

/// Mint already registered on this deployment, plus the sender's token account.
pub struct SplAsset {
    pub asset_id: u64,
    pub mint: Pubkey,
    pub user_token_account: Pubkey,
    pub token_program: Pubkey,
}

/// Load the usual example setup, then the registered SPL mint from `ZOLANA_SPL_MINT`.
pub fn setup_spl() -> Result<(SetupContext, SplAsset)> {
    let ctx = setup()?;
    let mint: Pubkey = std::env::var("ZOLANA_SPL_MINT")
        .map_err(|_| anyhow!("set ZOLANA_SPL_MINT"))?
        .parse()
        .map_err(|e| anyhow!("parse ZOLANA_SPL_MINT: {e}"))?;

    let rpc = SolanaRpc::new(ctx.rpc_url.clone());
    let registry_pda = pda::spl_asset_registry(&mint);
    let registry_address = Address::new_from_array(registry_pda.to_bytes());
    let account = rpc.get_account(registry_address)?.ok_or_else(|| {
        anyhow!("SPL asset registry not found for mint {mint}; set ZOLANA_SPL_MINT to a registered mint")
    })?;
    let registry = SplAssetRegistry::from_account_bytes(&account.data)
        .map_err(|e| anyhow!("parse SPL asset registry for mint {mint}: {e:?}"))?;

    let payer_path = std::env::var("ZOLANA_PAYER_KEYPAIR")
        .unwrap_or_else(|_| "~/.config/solana/id.json".to_string());
    let payer_path = shellexpand::tilde(&payer_path).into_owned();
    let payer =
        read_keypair_file(&payer_path).map_err(|e| anyhow!("load payer {payer_path}: {e}"))?;

    let sender_solana = ctx.sender.to_solana_keypair()?;
    let (_sig, user_token_account) =
        create_associated_token_account(&rpc, &payer, &sender_solana.pubkey(), &mint)
            .map_err(|e| anyhow!("create sender ATA for mint {mint}: {e}"))?;

    let token_program = pda::spl_token_program_id();
    let payer_ata = pda::associated_token_address(&payer.pubkey(), &mint);
    let payer_ata_address = Address::new_from_array(payer_ata.to_bytes());
    let payer_token = rpc.get_account(payer_ata_address)?.ok_or_else(|| {
        anyhow!("payer ATA {payer_ata} is missing; the payer must hold mint {mint}")
    })?;
    let payer_amount = token_account_amount(&payer_token.data)?;
    if payer_amount < SPL_DEPOSIT_AMOUNT {
        return Err(anyhow!(
            "payer ATA {payer_ata} has {payer_amount}, need {SPL_DEPOSIT_AMOUNT}"
        ));
    }

    let transfer_ix = spl_token_transfer(
        token_program,
        payer_ata,
        user_token_account,
        payer.pubkey(),
        SPL_DEPOSIT_AMOUNT,
    );
    rpc.create_and_send_transaction(&[transfer_ix], payer.pubkey(), &[&payer])
        .map_err(|e| anyhow!("fund sender ATA from payer ATA {payer_ata}: {e}"))?;

    Ok((
        ctx,
        SplAsset {
            asset_id: registry.asset_id,
            mint,
            user_token_account,
            token_program,
        },
    ))
}

fn token_account_amount(data: &[u8]) -> Result<u64> {
    let amount = data
        .get(64..72)
        .ok_or_else(|| anyhow!("token account data too short"))?;
    Ok(u64::from_le_bytes(amount.try_into()?))
}

fn spl_token_transfer(
    token_program: Pubkey,
    source: Pubkey,
    destination: Pubkey,
    owner: Pubkey,
    amount: u64,
) -> Instruction {
    let mut data = Vec::with_capacity(9);
    data.push(3);
    data.extend_from_slice(&amount.to_le_bytes());
    Instruction {
        program_id: token_program,
        accounts: vec![
            AccountMeta::new(source, false),
            AccountMeta::new(destination, false),
            AccountMeta::new_readonly(owner, true),
        ],
        data,
    }
}
