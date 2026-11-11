# Private smart account (client only)

A Squads smart account whose vault holds private balances in the zolana
shielded pool (SPP). Every signer views them through a shared viewing key,
which the signers encrypt to each other in a key record. The vault's data
record in a minimal data registry program stores the current key record. No
proof checks a key record. See [`design.md`](design.md) for the flows and
their limitations.

> [!WARNING]
> **Not for production.** An example, not audited code. Nothing checks that a
> key record's ciphertexts open for every signer; see the limitations in
> `design.md`. The data registry program is not deployed; the localnet loads
> it at `Eoxe4CcAxAjM4FE4YcR889kDVDmZvr9txS74LQ8ybHdU`.

## Layout

| File                                                                       |                                                                                                                                                             |
| -------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------- |
| [`program/src/lib.rs`](program/src/lib.rs)                                 | The data registry: a stripped-down user registry whose only instruction, `set_data`, lets an owner replace the data at the PDA `[b"data", owner]`.          |
| [`src/squads.ts`](src/squads.ts)                                           | Squads PDAs, `createSmartAccount`, `executeTransactionSyncV2`, `executeSettingsTransactionSync`, the `Settings` decoder and the sync consensus check.       |
| [`src/keyRecord.ts`](src/keyRecord.ts)                                     | The key record layout (`173 + 97 · N` bytes) and its encryption.                                                                                            |
| [`src/registry.ts`](src/registry.ts)                                       | The vault's registered key and data record: the current key record, the earlier ones from the `set_data` history, `register`, `update_keys` and `set_data`. |
| [`src/smartAccount.ts`](src/smartAccount.ts)                               | The Create, Register, Rotate and Spend flows.                                                                                                               |
| [`src/bytes.ts`](src/bytes.ts)                                             | The little-endian byte reader and writer.                                                                                                                   |
| [`test/localnet/smartAccount.test.ts`](test/localnet/smartAccount.test.ts) | All flows end to end on a localnet.                                                                                                                         |

## Key record encryption

Each ciphertext is `AES-256-CTR(key, iv = 0, plaintext)` with
`key = HKDF-SHA256(X25519(e, R), salt = E || R, info)`, where `e`/`E` is the
record's ephemeral keypair and `R` the recipient's tagged viewing public key.
The info is `KEY_INFO` for the shared viewing secret (to each signer) and
`NULLIFIER_INFO` for the 31-byte nullifier secret (to the shared viewing
key). A signer checks that the secrets it decrypts match the record's
`shared_viewing_pubkey` and `nullifier_pubkey`. Only signers receive the
shared viewing secret; anyone handed it would also open the nullifier secret
and see which vault UTXOs are spent, though spending still needs the vault's
signature.

## Transactions

- The vault is the SPP owner signer and fee payer and pays the rent of its
  user registry record and data record, all inside Squads CPIs, so it needs
  SOL. Squads strips the signer flag of its own members inside the CPI, so a
  member cannot pay there.
- Register sends the user registry `register` and the data registry
  `set_data` of key record 0 in one Squads transaction. Rotate sends
  `update_keys` and `set_data` of the new record in one Squads transaction
  before the settings change, so both run under the current signers and
  threshold. The deployed user registry takes only the viewing key in
  `update_keys`, so the client builds that instruction itself.
- Every transaction is sync: `time_lock` is 0. The tests send version 1
  transactions (4096 bytes), which carry a whole key record.
- `createSmartAccount` returns key record 0, which the creator hands to the
  other signers for Register. It derives the Settings address from the Squads
  account counter, so if another account is created first the create fails
  and is built again.
- Rotate reads kept signers' viewing keys from their user registry records,
  so every kept signer must be registered.
- An approver calls `checkOwnEntry` on the record before co-signing a
  Register, or a rotate it stays a signer of.
- `set_data` replaces the data, so the data record holds only the current key
  record. `recoverVaultKeys` reads the earlier ones from the `set_data`
  instructions in the data record's transaction history
  (`getSignaturesForAddress`, then `getTransaction`), so the RPC must keep
  that history. It returns the viewing keys of every record the member has
  an entry in; the SDK's `syncWallet` tries each of them on the vault's
  outputs, so UTXOs sent before a rotate stay visible.

## Run

Install Node.js 24+, pnpm and the Anza CLI (`cargo build-sbf`), then:

```bash
pnpm install
pnpm test                    # unit tests
bash scripts/localnet.sh     # start the cli-v0.10.0 release localnet
pnpm test:localnet           # create, register, spend and rotate
bash scripts/localnet.sh --stop
```

`scripts/localnet.sh` builds the data registry program, installs the
`cli-v0.10.0` CLI under `target/sa-client`, and runs `zolana dev start`, which
loads the deployed shielded pool, user registry and the Squads smart-account
program the release pins, plus the data registry. It adds the Squads
`ProgramConfig` account the release snapshot leaves out. Ports default to RPC
18899, Photon 18784 and prover 13301; `SA_RPC_URL`, `SA_INDEXER_URL` and
`SA_PROVER_URL` point the tests elsewhere.
