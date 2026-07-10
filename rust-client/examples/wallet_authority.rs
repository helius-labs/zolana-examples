use anyhow::{anyhow, Result};
use rust_client_example::{create_test_recipient, env_config, setup_funded_wallet};
use solana_address::Address;
use solana_pubkey::Pubkey;
use solana_signer::Signer;
use zolana_client::{
    create_transfer_sync, get_private_token_balances, sync_wallet, AnonymousRecipientSlot,
    ApprovalRequest, ClientError, ConfidentialRecipientSlot, CreateTransfer, EncryptedTransfer,
    P256Signature, SyncWalletAuthority, ZolanaClient,
};
use zolana_keypair::{
    viewing_key::ViewTag, NullifierKey, ShieldedAddress, ShieldedKeypair, ViewingKey,
};
use zolana_transaction::serialization::{
    anonymous::AnonymousTransferSenderPlaintext, confidential::TransferSenderPlaintext,
};

/// Stands in for the wallet's existing secure key system. The keypair stays
/// private; the SDK only ever calls through the Privacy Interface below and
/// never receives key material directly.
struct WalletKeyManager {
    keys: ShieldedKeypair,
}

impl WalletKeyManager {
    fn new(keys: ShieldedKeypair) -> Self {
        Self { keys }
    }
}

/// The Privacy Interface: each hook delegates to the keys held inside the
/// wallet, so key access stays behind the wallet boundary.
impl SyncWalletAuthority for WalletKeyManager {
    fn shielded_address(&self, owner_pubkey: Pubkey) -> Result<ShieldedAddress, ClientError> {
        SyncWalletAuthority::shielded_address(&self.keys, owner_pubkey)
    }

    fn encrypt_confidential_transfer(
        &self,
        owner_pubkey: Pubkey,
        first_nullifier: &[u8; 32],
        sender_tag: ViewTag,
        sender: &TransferSenderPlaintext,
        recipients: &[ConfidentialRecipientSlot],
    ) -> Result<EncryptedTransfer, ClientError> {
        SyncWalletAuthority::encrypt_confidential_transfer(
            &self.keys,
            owner_pubkey,
            first_nullifier,
            sender_tag,
            sender,
            recipients,
        )
    }

    fn encrypt_anonymous_transfer(
        &self,
        owner_pubkey: Pubkey,
        first_nullifier: &[u8; 32],
        sender_view_tag: ViewTag,
        sender: &AnonymousTransferSenderPlaintext,
        recipients: &[AnonymousRecipientSlot],
    ) -> Result<EncryptedTransfer, ClientError> {
        SyncWalletAuthority::encrypt_anonymous_transfer(
            &self.keys,
            owner_pubkey,
            first_nullifier,
            sender_view_tag,
            sender,
            recipients,
        )
    }

    // Where the wallet shows its normal send approval UI before the SDK signs.
    fn request_user_approval(&self, request: ApprovalRequest) -> Result<(), ClientError> {
        println!("approval requested: {}", request.summary);
        Ok(())
    }

    // Scoped disclosure: the wallet signs only the per-transaction message
    // hash; the signing key never leaves the key manager.
    fn sign_p256(
        &self,
        owner_pubkey: Pubkey,
        message_hash: &[u8; 32],
    ) -> Result<P256Signature, ClientError> {
        SyncWalletAuthority::sign_p256(&self.keys, owner_pubkey, message_hash)
    }

    // Scoped disclosure: releases the nullifier key that goes into the
    // per-transaction prover witness sent to the proving provider.
    fn spend_nullifier_key(&self, owner_pubkey: Pubkey) -> Result<NullifierKey, ClientError> {
        SyncWalletAuthority::spend_nullifier_key(&self.keys, owner_pubkey)
    }
}

fn main() -> Result<()> {
    // Load the fee payer and API key from .env, then connect to devnet.
    let (payer, api_key) = env_config()?;
    let sender_keypair = ShieldedKeypair::from_ed25519(&payer, ViewingKey::new())?;
    let rpc = ZolanaClient::devnet(&api_key);

    // Test setup: a test asset, the sender's funded private wallet, and the
    // recipient's private wallet.
    let sender = setup_funded_wallet(&rpc, &payer, rpc.tree(), &sender_keypair, 10_000)?;
    let mut recipient = create_test_recipient(&rpc, &payer, sender.registry)?;

    // Hand the keys to the wallet's key manager; from here the SDK only sees
    // the authority.
    let key_manager = WalletKeyManager::new(sender_keypair);

    // Build and sign the private transfer through the authority. If the
    // recipient does not have a private wallet, the SDK resolves to a
    // private-to-public withdrawal.
    let sender_address = Address::new_from_array(payer.pubkey().to_bytes());
    let mint = Address::new_from_array(sender.asset.mint.to_bytes());
    let transfer = create_transfer_sync(CreateTransfer {
        rpc: &rpc,
        wallet: &sender.wallet,
        authority: &key_manager,
        owner_pubkey: Pubkey::default(),
        payer: sender_address,
        recipient: recipient.keypair.pubkey(),
        asset: mint, // for SOL: SOL_MINT
        amount: 4_000,
        memo: Some(b"thanks".to_vec()), // encrypted; only the recipient can read it
    })?;
    if transfer.recipient.is_public_withdrawal() {
        return Err(anyhow!(
            "expected a private transfer, got a public withdrawal"
        ));
    }

    // Prove and submit the private transfer. The proof shows the sender owns the
    // balance being spent and has not already spent it.
    let signature = rpc.submit(&payer).execute(
        transfer.signed,
        transfer.recipient.withdrawal().cloned(),
        transfer.wait_tag,
    )?;

    // Sync the recipient's private balance.
    sync_wallet(&mut recipient.wallet, &rpc)?;
    let balance = get_private_token_balances(&recipient.wallet)?;

    println!("ok private transfer signature={signature} recipient_private_balance={balance:?}");
    Ok(())
}
