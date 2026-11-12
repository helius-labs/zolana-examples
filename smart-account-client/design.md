# Private Smart Account (Client)

A Squads smart account holds private balances in the zolana shielded pool (SPP). Every signer can view them through a shared viewing key. Whenever the account is created or its signers change, the signer making the change uploads a key record to the key record store, with that key encrypted to every signer. No program or proof checks the encryption.

## Flows

### 1. Create

1. **Derive.** Each initial signer derives its `ShieldedKeypair` (nullifier key and viewing key) from its Solana keypair.
2. **Assemble signers.** The creator collects the initial signers' viewing public keys, from the user registry or from the signers directly, and picks the threshold.
3. **Encrypt.** The creator generates a random shared viewing keypair and a random nullifier secret. Under one ephemeral key, it encrypts the shared viewing secret to every signer's viewing public key and the nullifier secret to the shared viewing public key.
4. **Create.** The creator submits Squads `createSmartAccount` with no settings authority and `time_lock = 0` and shares the initial key record with the other signers.
5. **Upload and check.** A rent payer uploads the key record in chunks into a fresh buffer. Each signer decrypts its entry and checks that the result belongs to the shared viewing public key. Its approval binds the exact record hash and the empty predecessor.
6. **Register.** A threshold of signers sends Squads `executeTransactionSyncV2` with an inner user registry `register` and storage `apply`, both signed by the vault. These register the vault's keys and publish its initial key record atomically.

### 2. Rotate

1. **Assemble signers.** The rotator picks the new signer set and threshold by adding, removing or keeping signers, and collects each added signer's viewing public key.
2. **Recover.** The rotator decrypts its entry in the current key record to get the shared viewing secret, then uses that to decrypt the nullifier secret and every old viewing secret in the record.
3. **Encrypt.** The rotator generates a new random shared viewing keypair. Under one new ephemeral key, it encrypts the new shared viewing secret to every viewing public key in the new set. It encrypts the unchanged nullifier secret and all previous viewing secrets, including the current one, to the new shared viewing public key. The old-key vector is ordered oldest first.
4. **Upload and approve.** A rent payer uploads the new key record in chunks. The old record stays active. A threshold of the current Squads signers co-signs the final transaction, binding the new record hash, current record address and head revision. Each approver that stays a signer checks its own entry, decrypts and verifies the old viewing keys, and checks their public keys against the previous record's complete history plus its current viewing key.
5. **Rotate.** The rotator submits one transaction with these instructions, in this order:
   - Squads `executeTransactionSyncV2` with an inner registry `update_keys` setting the new `shared_viewing_pubkey` and storage `apply` publishing the uploaded record;
   - Squads `executeSettingsTransactionSync` with the add-signer, change-threshold and remove-signer actions.

### 3. Spend

1. **Recover.** The spender, a current signer, decrypts its entry in the current key record, then decrypts the nullifier secret and all old viewing secrets. Newly added signers receive the same full history as other current signers.
2. **Sync.** The spender finds the vault's UTXOs by their view tag, which is the vault address, and tries the current and old viewing keys to decrypt them.
3. **Build.** The spender picks the input UTXOs and builds the outputs:
   - each recipient's output goes to the `owner_hash` and viewing public key in that recipient's `UserRecord`;
   - change goes back to the vault, encrypted to the shared viewing public key.
4. **Prove.** The spender generates the SPP `transact` proof, with the nullifier secret as a private input and the vault as the owner signer.
5. **Approve.** A threshold of signers with vote permission co-signs the spend transaction. Together they must hold the initiate, vote and execute permissions.
6. **Spend.** The spender submits Squads `executeTransactionSyncV2` with an inner SPP `transact`. Squads checks the approvals and signs `transact` as the vault.

## Key Record

The storage program stores opaque bytes; it does not parse or validate KeyRecord. New records are uploaded in chunks into a separate buffer while the current record stays active. A threshold of signers atomically applies that buffer together with the user-registry update and any Squads settings changes. Apply checks the approved bytes' hash, complete length, expected predecessor and head revision. It seals the replacement, advances the head and revision, closes the superseded record and refunds its balance to the apply rent payer. All of these changes are atomic. Current signers recover every old viewing key directly from the latest record. See [the storage protocol](program/STORAGE.md).

A client checks that the current record's `shared_viewing_pubkey` equals the vault's registered `viewing_pubkey` and its entry signers equal the Squads `Settings.signers`. Approvers check their own encrypted entries before signing. The storage program stores opaque bytes and does not validate the ciphertexts.

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
    /// Every old viewing secret encrypted to shared_viewing_pubkey, oldest first.
    pub old_viewing_keys: Vec<EncryptedViewingKey>,
}

pub struct KeyEntry {
    /// The signer's Solana public key, as in Squads Settings.signers.
    pub signer: Pubkey,
    /// The signer's tagged X25519 viewing public key.
    pub viewing_pubkey: [u8; 33],
    /// AES-256-CTR encryption of the 32-byte shared viewing secret to `viewing_pubkey`.
    pub ciphertext: [u8; 32],
}

pub struct EncryptedViewingKey {
    /// Tagged public key used to verify the decrypted old viewing secret.
    pub viewing_pubkey: [u8; 33],
    /// AES-256-CTR encryption of the 32-byte secret to shared_viewing_pubkey.
    pub ciphertext: [u8; 32],
}
```

A key record is `177 + 97 · N + 65 · H` bytes for N signers and H old viewing keys. The client requires H to equal the epoch. Historical key i uses HKDF info `OLD_KEY_INFO || LE32(i)`, distinct from the signer and nullifier domains and from every other old-key index. Decryption checks every secret against its advertised public key. These checks are entirely client-side.

## Limitations

1. No check of the ciphertexts.
2. Reading published records requires an RPC that serves their account state.
3. An incomplete or abandoned upload locks rent until its buffer signer cancels it.
4. Nothing enforces the rotate flow.
5. The nullifier key doesn't rotate.
6. A removed signer keeps the old shared viewing secret and the nullifier secret.
7. The creator learns both secrets.
8. A signer's own key update isn't picked up.
9. Membership is public.
10. All transactions are sync.
11. The signer count is capped.
12. Every current signer, including newly added signers, can view all earlier balances. Removed signers retain access to the history they already received.
