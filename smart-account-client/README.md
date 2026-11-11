# Private smart account (client only)

A Squads smart account whose vault holds private balances in the zolana
shielded pool (SPP). Every signer views them through a shared viewing key,
which the signers encrypt to each other in a key record kept in the vault's
user registry record. No program of this example runs on chain and
no proof checks a key record. See [`design.md`](design.md) for the flows and
their limitations.

> [!WARNING]
> **Not for production.** An example, not audited code. Nothing checks that a
> key record's ciphertexts open for every signer; see the limitations in
> `design.md`. It builds on an unreleased user registry with `set_data`
> (helius-labs/privacy-rings#613).

## Layout

| File                                                                       |                                                                                                                                                       |
| -------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------- |
| [`src/squads.ts`](src/squads.ts)                                           | Squads PDAs, `createSmartAccount`, `executeTransactionSyncV2`, `executeSettingsTransactionSync`, the `Settings` decoder and the sync consensus check. |
| [`src/keyRecord.ts`](src/keyRecord.ts)                                     | The key record layout (`173 + 97 · N` bytes) and its encryption.                                                                                      |
| [`src/registry.ts`](src/registry.ts)                                       | The vault's registry record: the current key record and the earlier ones from its `set_data` history, and `register`, `update_keys` and `set_data`.   |
| [`src/smartAccount.ts`](src/smartAccount.ts)                               | The Create, Register, Rotate and Spend flows.                                                                                                         |
| [`src/bytes.ts`](src/bytes.ts)                                             | The little-endian byte reader and writer.                                                                                                             |
| [`test/localnet/smartAccount.test.ts`](test/localnet/smartAccount.test.ts) | All flows end to end on a localnet.                                                                                                                   |

## Key record encryption

Each ciphertext is `AES-256-CTR(key, iv = 0, plaintext)` with
`key = HKDF-SHA256(X25519(e, R), salt = E || R, info)`, where `e`/`E` is the
record's ephemeral keypair and `R` the signer's tagged viewing public key.
The info is `KEY_INFO` for the shared viewing secret (to each signer) and
`NULLIFIER_INFO` for the 31-byte nullifier secret (to the shared viewing
key). A signer checks that the secrets it decrypts match the record's
`shared_viewing_pubkey` and `nullifier_pubkey`. Only signers receive the
shared viewing secret; anyone handed it would also open the nullifier secret
and see which vault UTXOs are spent, though spending still needs the vault's
signature.

## Transactions

- The vault is the SPP owner signer and fee payer and the registry rent
  payer, all inside Squads CPIs, so it needs SOL. Squads strips the signer
  flag of its own members inside the CPI, so a member cannot pay there.
- Register sends `register` and `set_data` of key record 0 in one Squads
  transaction. Rotate sends `update_keys` and `set_data` of the new record
  in one Squads transaction before the settings change, so both run under the
  current signers and threshold.
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
- `set_data` replaces the registry data, so the registry record holds only
  the current key record. `recoverVaultKeys` reads the earlier ones from the
  `set_data` instructions in the record account's transaction history
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

`@heliuslabs/spr` is pinned to a privacy-rings commit of
helius-labs/privacy-rings#613, the first SDK whose registry decoder accepts
record data. `scripts/localnet.sh` builds the user registry of that same
commit under `target/sa-client/registry`, installs the `cli-v0.10.0` CLI, and
runs `zolana dev start`, which loads the deployed shielded pool and the Squads
smart-account program the release pins, then the built registry over the
release one. It adds the Squads `ProgramConfig` account the release snapshot
leaves out. Ports default to RPC 18899, Photon 18784 and prover 13301;
`SA_RPC_URL`, `SA_INDEXER_URL` and `SA_PROVER_URL` point the tests elsewhere.
The commit is bumped by hand in `package.json`, `pnpm-workspace.yaml` and the
lockfile; `sync-sdk.yml` does not touch this package.
