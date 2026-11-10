# Private smart account (client only)

A Squads smart account whose vault holds private balances in the zolana
shielded pool (SPP). Every signer views them through a shared viewing key,
which the signers encrypt to each other in a key record kept off chain. No
program of this example runs on chain and no proof checks the key record.
See [`design.md`](design.md) for the flows and their limitations.

> [!WARNING]
> **Not for production.** An example, not audited code. Nothing checks that a
> key record's ciphertexts open for every signer; see the limitations in
> `design.md`.

## Layout

| File                                                                       |                                                                                                                                                       |
| -------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------- |
| [`src/squads.ts`](src/squads.ts)                                           | Squads PDAs, `createSmartAccount`, `executeTransactionSyncV2`, `executeSettingsTransactionSync`, the `Settings` decoder and the sync consensus check. |
| [`src/keyRecord.ts`](src/keyRecord.ts)                                     | The key record layout (`173 + 97 · N` bytes), its encryption and the current-record rule.                                                             |
| [`src/store.ts`](src/store.ts)                                             | The key record store interface, an in-memory store, and the current-record lookup.                                                                    |
| [`src/smartAccount.ts`](src/smartAccount.ts)                               | The Create, Register, Rotate and Spend flows.                                                                                                         |
| [`test/localnet/smartAccount.test.ts`](test/localnet/smartAccount.test.ts) | All flows end to end on a localnet.                                                                                                                   |

## Key record encryption

Each ciphertext is `AES-256-CTR(key, iv = 0, plaintext)` with
`key = HKDF-SHA256(X25519(e, R), salt = E || R, info)`, where `e`/`E` is the
record's ephemeral keypair and `R` the recipient's tagged viewing public key.
The info is `KEY_INFO` for the shared viewing secret (to each signer) and
`NULLIFIER_INFO` for the 31-byte nullifier secret (to the shared viewing
key). A signer checks that the secrets it decrypts match the record's
`shared_viewing_pubkey` and `nullifier_pubkey`.

## Transactions

- The vault is the SPP owner signer and fee payer and the registry rent
  payer, all inside Squads CPIs, so it needs SOL. Squads strips the signer
  flag of its own members inside the CPI, so a member cannot pay there.
- Rotate sends the registry `update_keys` before the settings change, so it
  runs under the current signers and threshold.
- Every transaction is sync: `time_lock` is 0.
- `createSmartAccount` and `rotateSmartAccount` return the key record without
  storing it. The caller uploads it after the approvals, then sends.
  `createSmartAccount` derives the Settings address from the Squads account
  counter, so if another account is created first the create fails and is
  built again.
- Rotate reads kept signers' viewing keys from their user registry records,
  never from the store, so every kept signer must be registered. Once the
  vault is registered, exactly one stored record may match the on-chain
  state; a store that serves a second one makes the lookup fail.
- An approver that stays a signer calls `checkOwnEntry` on the new record
  before co-signing a rotate.

## Run

Install Node.js 24+ and pnpm, then:

```bash
pnpm install
pnpm test                    # unit tests
bash scripts/localnet.sh     # start the cli-v0.10.0 release localnet
pnpm test:localnet           # create, register, spend and rotate
bash scripts/localnet.sh --stop
```

`scripts/localnet.sh` installs the `cli-v0.10.0` CLI under `target/sa-client`
and runs `zolana dev start`, which loads the deployed shielded pool, user
registry and the Squads smart-account program the release pins. It adds the
Squads `ProgramConfig` account the release snapshot leaves out. Ports default
to RPC 18899, Photon 18784 and prover 13301; `SA_RPC_URL`, `SA_INDEXER_URL`
and `SA_PROVER_URL` point the tests elsewhere.
