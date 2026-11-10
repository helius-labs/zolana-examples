# Private Smart Account

A Squads smart account holds private balances in the zolana shielded pool (SPP). Every signer can view them through a shared viewing key, and a key-rotation program proves, at creation and on every membership change, that the shared viewing key is encrypted to all signers.

## Glossary

## Flows

### 1. Create

1. **Derive.** Each initial signer derives its `ShieldedKeypair` (nullifier key and viewing key) from its Solana keypair.
2. **Assemble signers.** The creator collects the initial signers' shielded addresses and picks the threshold. Each signer's viewing public key must be either registered in the user registry or confirmed by that signer co-signing `create`.
3. **Encrypt.** The creator generates a random shared viewing keypair and encrypts the shared viewing secret to every signer's viewing public key under one ephemeral key.
4. **Prove.** The creator proves that every ciphertext encrypts the shared viewing secret to its signer's viewing public key, and that the secret belongs to the shared viewing public key.
5. **Create.** The creator submits `create`. The key-rotation program:
   - checks each signer's viewing public key against the user registry or that signer's signature;
   - verifies the proof;
   - creates the Squads smart account with its PDA as settings authority;
   - writes the key record account.

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

## Instructions

### 1. create

### 2. rotate

## Spending

## Limitations

1. we will want to register the smart account and rotate the nullifier key too, we will need to change the registry program to be able to rotate the nullifier key.
