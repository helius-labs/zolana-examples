# Private Smart Account (Client)

A Squads smart account holds private balances in the zolana shielded pool (SPP). Every signer can view them through a shared viewing key. Whenever the account is created or its signers change, the signer making the change uploads a key record to the key record store, with that key encrypted to every signer. No program or proof checks the encryption.

## Flows

### 1. Create

1. **Derive.** Each initial signer derives its `ShieldedKeypair` (nullifier key and viewing key) from its Solana keypair.
2. **Assemble signers.** The creator collects the initial signers' viewing public keys, from the user registry or from the signers directly, and picks the threshold.
3. **Encrypt.** The creator generates a random shared viewing keypair and a random nullifier secret. Under one ephemeral key, it encrypts the shared viewing secret to every signer's viewing public key and the nullifier secret to the shared viewing public key.
4. **Create.** The creator uploads the key record to the key record store, then submits Squads `createSmartAccount` with no settings authority and `time_lock = 0`.
5. **Check.** Each signer fetches the key record from the key record store, decrypts its entry, and checks that the result belongs to the shared viewing public key.
6. **Register.** A threshold of signers sends Squads `executeTransactionSyncV2` with an inner user registry `register`, signed by the vault. It registers the vault's `nullifier_pubkey` and `shared_viewing_pubkey`.

### 2. Rotate

1. **Assemble signers.** The rotator picks the new signer set and threshold by adding, removing or keeping signers, and collects each added signer's viewing public key.
2. **Recover.** The rotator decrypts its entry in the current key record to get the shared viewing secret, then uses that to decrypt the nullifier secret.
3. **Encrypt.** The rotator generates a new random shared viewing keypair. Under one new ephemeral key, it encrypts the new shared viewing secret to every viewing public key in the new set, and the unchanged nullifier secret to the new shared viewing public key.
4. **Approve.** A threshold of the current Squads signers with vote permission co-signs the rotate transaction. Each approver that stays a signer checks its own entry first.
5. **Rotate.** The rotator uploads the new key record to the key record store, then submits one transaction with these instructions, in this order:
   - if the vault is registered, Squads `executeTransactionSyncV2` with an inner registry `update_keys` setting the new `shared_viewing_pubkey`;
   - Squads `executeSettingsTransactionSync` with the add-signer, change-threshold and remove-signer actions.

### 3. Spend

1. **Recover.** The spender, a current signer, decrypts its entry in the current key record to get the shared viewing secret, then uses that to decrypt the nullifier secret.
2. **Sync.** The spender finds the vault's UTXOs by their view tag, which is the vault address, and decrypts them with the shared viewing secret.
3. **Build.** The spender picks the input UTXOs and builds the outputs:
   - each recipient's output goes to the `owner_hash` and viewing public key in that recipient's `UserRecord`;
   - change goes back to the vault, encrypted to the shared viewing public key.
4. **Prove.** The spender generates the SPP `transact` proof, with the nullifier secret as a private input and the vault as the owner signer.
5. **Approve.** A threshold of signers with vote permission co-signs the spend transaction. Together they must hold the initiate, vote and execute permissions.
6. **Spend.** The spender submits Squads `executeTransactionSyncV2` with an inner SPP `transact`. Squads checks the approvals and signs `transact` as the vault.

## Key Record

The key record store keeps every key record it receives. A client takes as current the record whose `shared_viewing_pubkey` equals the vault's registered `viewing_pubkey` and whose entry signers equal the Squads `Settings.signers`. Before the vault is registered, only the signer match applies. A record whose transaction never landed matches neither, so the client does not trust the store to say which record is current.

```rust
/// Borsh-serialized key record, stored in the key record store.
pub struct KeyRecord {
    /// The Squads Settings account this record belongs to.
    pub settings: Pubkey,
    /// 0 at create, incremented by every rotate.
    pub epoch: u64,
    /// Tagged X25519 public key of the shared viewing secret.
    pub shared_viewing_pubkey: [u8; 33],
    /// Tagged X25519 public key of the ephemeral secret shared by all ciphertexts.
    pub ephemeral_pubkey: [u8; 33],
    /// Poseidon(nullifier_secret); rotate keeps it unchanged.
    pub nullifier_pubkey: [u8; 32],
    /// AES-256-CTR encryption of the 31-byte nullifier secret to `shared_viewing_pubkey`.
    pub nullifier_ciphertext: [u8; 31],
    /// One entry per Squads signer; the entry signers equal Settings.signers.
    pub entries: Vec<KeyEntry>,
}

pub struct KeyEntry {
    /// The signer's Solana public key, as in Squads Settings.signers.
    pub signer: Pubkey,
    /// The signer's tagged X25519 viewing public key.
    pub viewing_pubkey: [u8; 33],
    /// AES-256-CTR encryption of the 32-byte shared viewing secret to `viewing_pubkey`.
    pub ciphertext: [u8; 32],
}
```

A key record is `173 + 97 · N` bytes for N signers.

## Limitations

1. No check of the ciphertexts.
2. The key record store can withhold records.
3. The store can swap the record before registration.
4. Nothing enforces the rotate flow.
5. The nullifier key doesn't rotate.
6. A removed signer keeps the old shared viewing secret and the nullifier secret.
7. The creator learns both secrets.
8. A signer's own key update isn't picked up.
9. Membership is public.
10. All transactions are sync.
11. The signer count is capped.
12. A signer added by a rotate cannot open UTXOs the vault received before it.
