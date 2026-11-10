# Private Smart Account

A Squads smart account holds private balances in the zolana shielded pool (SPP). Every signer can view them through a shared viewing key, and a key-rotation program proves, at creation and on every membership change, that the shared viewing key is encrypted to all signers.

## Glossary

## Flows

### 1. Create

1. **Derive.** Each initial signer derives its `ShieldedKeypair` (nullifier key and viewing key) from its Solana keypair.
2. **Assemble signers.** The creator collects the initial signers' shielded addresses and picks the threshold. Each signer's viewing public key must be either registered in the user registry or confirmed by that signer co-signing `create`.
3. **Encrypt.** The creator generates a random shared viewing keypair and a random nullifier secret. Under one ephemeral key, it encrypts the shared viewing secret to every signer's viewing public key and the nullifier secret to the shared viewing public key.
4. **Prove.** The creator proves that every ciphertext encrypts the shared viewing secret to its signer's viewing public key, that the secret belongs to the shared viewing public key, and that the nullifier ciphertext encrypts the secret behind `nullifier_pubkey` to the shared viewing public key.
5. **Create.** The creator submits `create`. The key-rotation program:
   - checks each signer's viewing public key against the user registry or that signer's signature;
   - verifies the proof;
   - creates the Squads smart account with its key record account as settings authority;
   - writes the key record account.
6. **Register.** A threshold of signers sends Squads `executeTransactionSyncV2` with an inner user registry `register`, signed by the vault. It registers the key record account's `nullifier_pubkey` and `shared_viewing_pubkey`.

### 2. Rotate

1. **Assemble signers.** A current signer (the rotator) picks the new signer set and threshold by adding, removing or keeping signers. Each added signer's viewing public key must be either registered in the user registry or confirmed by that signer co-signing `rotate`.
2. **Recover.** The rotator decrypts its entry in the key record account to get the current shared viewing secret, then uses it to decrypt the nullifier secret.
3. **Encrypt.** The rotator generates a new random shared viewing keypair. Under one new ephemeral key, it encrypts the new shared viewing secret to every viewing public key in the new set and the unchanged nullifier secret to the new shared viewing public key.
4. **Prove.** The rotator proves that:
   - every ciphertext encrypts the new shared viewing secret to its signer's viewing public key;
   - the secret belongs to the new shared viewing public key;
   - the nullifier ciphertext encrypts the secret behind the stored `nullifier_pubkey`;
   - the proof builds on the current record, because its `OldStateHash` equals the stored `state_hash`.
5. **Approve.** A threshold of the current Squads signers with vote permission co-signs the `rotate` transaction.
6. **Rotate.** The rotator submits `rotate`. The key-rotation program:
   - checks that enough current Squads signers with vote permission signed to meet the threshold;
   - checks each kept signer's viewing public key against its key record entry, and each added signer's against the user registry or that signer's signature;
   - verifies the proof with the stored `state_hash` as `OldStateHash` and the stored `nullifier_pubkey`;
   - updates the Squads signers and threshold as settings authority;
   - if the vault's `UserRecord` exists, CPIs Squads `executeTransactionSyncV2` with an inner registry `update_keys` setting the new `shared_viewing_pubkey`;
   - overwrites the key record account with the new keys and ciphertexts, the new `state_hash`, and `epoch + 1`.

### Properties

## Constants

## Key Record Account

One PDA per smart account, at `[b"key_record", settings]` under the key-rotation program, where `settings` is the Squads `Settings` account. It holds the current shared viewing public key, one ciphertext of the shared viewing secret per signer, and the shared nullifier secret encrypted to the shared viewing key.

```rust
/// PDA `[b"key_record", settings]` under the key-rotation program.
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
    /// Public input hash of the proof that wrote this record; the next rotate
    /// proof takes it as `OldStateHash`.
    pub state_hash: [u8; 32],
    /// One entry per Squads signer; the entry signers equal Settings.signers.
    pub entries: Vec<KeyEntry>,
    pub bump: u8,
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

The set of `entries[i].signer` always equals the Squads signer set. `create` and `rotate` write the whole record. The account is reallocated when the number of signers changes, and its size is `215 + 97 · N` bytes for N signers.

## Key Encryption Proof

`create` and `rotate` verify the same gnark Groth16 proof. It shows that every signer can recover the shared viewing secret and that every signer who does can recover the nullifier secret. It reads no UTXOs. The circuit is compiled once for each signer count N, from 1 to `MAX_SIGNERS`, and the program picks the verifying key by the number of entries.

**Private inputs:** `viewing_secret` (32 bytes), `ephemeral_secret` (32 bytes), `nullifier_secret` (field element).

**Public input:** one `public_input_hash`, the hash chain over:

```
old_state_hash,                                  // 0 at create; the stored state_hash at rotate
shared_viewing_pubkey,
ephemeral_pubkey,
(viewing_pubkey_i, hash(ciphertext_i))  for i in 0..N,
nullifier_pubkey,
hash(nullifier_ciphertext)
```

The program recomputes this chain from the instruction data and the key record account, then stores the result as the record's new `state_hash`.

**The proof checks:**

1. `shared_viewing_pubkey` is the tagged X25519 public key of `viewing_secret`, and `ephemeral_pubkey` is the tagged X25519 public key of `ephemeral_secret`.
2. For each signer i, `ciphertext_i` is the AES-256-CTR encryption of `viewing_secret`. The key is derived from `ECDH(ephemeral_secret, viewing_pubkey_i)`, both public keys, and `KEY_INFO`. Each `viewing_pubkey_i` is canonical, and the ECDH result is not the identity.
3. `nullifier_pubkey = Poseidon(nullifier_secret)`.
4. `nullifier_ciphertext` is the AES-256-CTR encryption of `nullifier_secret` as 31 big-endian bytes, under the key derived from `ECDH(ephemeral_secret, shared_viewing_pubkey)` and `NULLIFIER_INFO`.

## Instructions

### 1. create

Creates a Squads smart account whose settings authority is its key record account, and writes the key record account. No funds move.

**Accounts**

1. `payer`: pays fees, rent and the Squads creation fee, and is the Squads `creator`; signer, writable.
2. `key_record`: the key record account `[b"key_record", settings]`; writable.
3. `settings`: the new Squads `Settings` account, `[b"smart_account", b"settings", smart_account_index + 1]` under the Squads program; writable.
4. `program_config`: the Squads `ProgramConfig`; writable.
5. `treasury`: the Squads treasury; writable.
6. `squads_program`, `system_program`.
7. `signer_accounts`: one account per entry, in entry order. It is either the signer itself as a transaction signer, or the signer's user registry `UserRecord`.

**Instruction data**

```rust
struct CreateIxData {
    threshold: u16,
    shared_viewing_pubkey: [u8; 33],
    ephemeral_pubkey: [u8; 33],
    nullifier_pubkey: [u8; 32],
    nullifier_ciphertext: [u8; 31],
    /// Sorted strictly ascending by `signer`, matching Squads' order.
    entries: Vec<SignerEntry>,
    proof: CompressedProof,
    key_record_bump: u8,
}

struct SignerEntry {
    signer: Pubkey,
    /// Squads permission mask (initiate, vote, execute).
    permissions: u8,
    viewing_pubkey: [u8; 33],
    ciphertext: [u8; 32],
}
```

**Program checks**

1. `settings` is the Squads settings PDA for `program_config.smart_account_index + 1`, and `key_record` is the PDA for `settings` with `key_record_bump`.
2. `entries` is non-empty, has at most `MAX_SIGNERS` entries, and is sorted strictly ascending by `signer`.
3. For each entry, its `signer_accounts` account is either:
   - the entry's `signer`, and it signed the transaction; or
   - the `UserRecord` PDA of `signer` under the user registry program, and its `viewing_pubkey` equals the entry's.
4. The program recomputes `public_input_hash` with `old_state_hash = 0` and verifies the proof against the verifying key for N = `entries.len()`.
5. The program CPIs Squads `create_smart_account` with:
   - `settings_authority = key_record`;
   - the entries' signers and permissions;
   - `threshold`;
   - `time_lock = 0`;
   - no rent collector and no memo.
6. The program creates `key_record` and writes `epoch = 0`, the keys and ciphertexts, and `state_hash = public_input_hash`.

### 2. rotate

Replaces the shared viewing key, re-encrypts it to the new signer set, and updates the Squads signers and threshold. The nullifier secret stays the same and is re-encrypted to the new shared viewing key.

**Accounts**

1. `payer`: pays fees and reallocation rent for `settings` and `key_record`, and receives refunds when they shrink; signer, writable.
2. `key_record`: the key record account; writable.
3. `settings`: the Squads `Settings` account of `key_record`; writable.
4. `squads_program`, `system_program`.
5. `vault`: the Squads smart account PDA at index 0.
6. `user_record`: the vault's user registry `UserRecord` PDA; writable.
7. `user_registry_program`.
8. `approvers`: the current signers approving the rotation; signers.
9. `added_signer_accounts`: one account per added signer, in entry order. It is either the signer itself as a transaction signer, or the signer's user registry `UserRecord`.

**Instruction data**

```rust
struct RotateIxData {
    /// Number of `approvers` accounts.
    num_approvers: u8,
    new_threshold: u16,
    shared_viewing_pubkey: [u8; 33],
    ephemeral_pubkey: [u8; 33],
    nullifier_ciphertext: [u8; 31],
    /// The new signer set, sorted strictly ascending by `signer`.
    entries: Vec<SignerEntry>,
    proof: CompressedProof,
}
```

**Program checks**

1. `key_record` is owned by the program and its `settings` field equals `settings`.
2. The `approvers` are distinct current Squads signers with vote permission, they all signed, and there are at least the current `threshold` of them.
3. `entries` is non-empty, has at most `MAX_SIGNERS` entries, and is sorted strictly ascending by `signer`.
4. For each entry:
   - if its signer is in the key record account (kept), its `viewing_pubkey` equals the stored one and its `permissions` equal the signer's in `Settings.signers`;
   - otherwise (added), its `added_signer_accounts` account is either the signer itself, having signed, or its `UserRecord` with a matching `viewing_pubkey`.
5. The program recomputes `public_input_hash` with the stored `state_hash` as `old_state_hash` and the stored `nullifier_pubkey`, then verifies the proof against the verifying key for N = `entries.len()`.
6. The program CPIs Squads as settings authority in this order:
   - `add_signer` for each added signer;
   - `change_threshold` if `new_threshold` differs;
   - `remove_signer` for each signer missing from `entries`.

    Squads checks its settings invariant after every call, and this order keeps each intermediate state valid. For example, going from 3-of-3 to 1-of-1 fails if the removals run before the threshold change.
7. `vault` is the Squads smart account PDA of `settings` at index 0, and `user_record` is the user registry PDA of `vault`. If `user_record` exists, the program CPIs Squads `executeTransactionSyncV2` with the `approvers` and an inner registry `update_keys` signed by `vault`, setting `shared_viewing_pubkey` and the stored `nullifier_pubkey`. If it is empty, the vault was never registered and the program skips the update.
8. The program resizes `key_record`, writes the new keys, ciphertexts and `nullifier_ciphertext`, sets `state_hash = public_input_hash`, and increments `epoch`.

### 3. forward_settings

Forwards one allowlisted Squads `*_as_authority` instruction to Squads, signed as `key_record`, once a threshold of current signers approves. Signer changes are not forwarded; signers change only through `rotate`.

**Accounts**

1. `payer`: pays fees and any rent the Squads instruction charges; signer, writable.
2. `key_record`: the key record account; read-only.
3. `settings`: the Squads `Settings` account of `key_record`; writable.
4. `squads_program`.
5. `approvers`: the current signers approving the change; signers.
6. `squads_accounts`: the inner instruction's account list, as Squads expects it. Every Squads authority instruction starts with `settings`, then `settings_authority`.

**Instruction data**

```rust
struct ForwardSettingsIxData {
    /// Number of `approvers` accounts.
    num_approvers: u8,
    /// The inner Squads instruction data, discriminator first.
    squads_ix_data: Vec<u8>,
}
```

**Allowed Squads instructions**


| Squads instruction                    | Effect                       |
| ------------------------------------- | ---------------------------- |
| `change_threshold_as_authority`       | Changes the threshold.       |
| `add_spending_limit_as_authority`     | Creates a spending limit.    |
| `remove_spending_limit_as_authority`  | Closes a spending limit.     |
| `set_archival_authority_as_authority` | Sets the archival authority. |
| `set_time_lock_as_authority` | Sets the time lock. |
| `set_new_settings_authority_as_authority` | Hands off the settings authority; `Pubkey::default()` makes the account autonomous. |


**Program checks**

1. `key_record` is owned by the program and its `settings` field equals `settings`.
2. The `approvers` are distinct current Squads signers with vote permission, they all signed, and there are at least `threshold` of them.
3. The first 8 bytes of `squads_ix_data` match an allowed Squads instruction.
4. `squads_accounts[0]` is `settings` and `squads_accounts[1]` is `key_record`.
5. The program CPIs Squads with `squads_ix_data` and `squads_accounts`, marking `key_record` as signer and signing with `[b"key_record", settings, bump]`. The key record account is not changed.

## Spending

## Limitations

1. The nullifier key does not rotate. The user registry's `update_keys` rejects a new `nullifier_pubkey`, so rotating it needs a registry program change. Every `rotate` still re-encrypts the unchanged nullifier secret to the new shared viewing key.
2. The key-rotation program does not check the registration in the Create flow. If the signers register a `nullifier_pubkey` other than the key record account's, every later `update_keys` fails and `rotate` cannot succeed.
3. A removed signer keeps the old shared viewing secret and the nullifier secret. 
4. The signer count is capped at `MAX_SIGNERS` 17.
5. All transactions are sync. A nonzero time lock blocks Squads sync execution, including the registry update in `rotate`.
6. Squads policies cannot be used while the key record account is the settings authority, since Squads creates them only through settings transactions. A threshold can hand off the settings authority through `forward_settings`; from then on, membership can change without a `rotate`.
7. Signer changes depend on the key-rotation program. If it is unavailable, signers cannot change; spending is unaffected.
8. `rotate` cannot change a kept signer's permissions. Changing them takes one rotation that removes the signer and another that adds it back.
9. If a signer updates its own viewing key in the user registry, the key record account keeps the old one until a `rotate` replaces the signer's entry.
10. The creator learns the shared viewing secret and the nullifier secret, even if it is not a signer.
11. Membership is public: Squads `Settings` and the key record account link the smart account to its signers' Solana keys and viewing public keys.