# Private smart account (client only)

A Squads smart account whose vault holds private balances in the zolana
shielded pool (SPP). Every signer views them through a shared viewing key,
which the signers encrypt to each other in a key record. A storage program
keeps the current key record behind a head PDA and closes superseded accounts. No
proof checks a key record. See [`design.md`](design.md) for the flows and
their limitations.

> [!WARNING]
> **Not for production.** An example, not audited code. Nothing checks that a
> key record's ciphertexts open for every signer; see the limitations in
> `design.md`. The data registry program is deployed on devnet and mainnet at
> `Stor9L8wQBAGcAcoUcADt5sWYYaEA65Ws2Cgt5UUqXj`.

## Layout

| File                                                                     |                                                                                                                                                       |
| ------------------------------------------------------------------------ | ----------------------------------------------------------------------------------------------------------------------------------------------------- |
| [`program/src/lib.rs`](program/src/lib.rs)                               | The storage program: chunked uploads, owner-authorized publication and cancellation of unpublished uploads.                                           |
| [`src/squads.ts`](src/squads.ts)                                         | Squads PDAs, `createSmartAccount`, `executeTransactionSyncV2`, `executeSettingsTransactionSync`, the `Settings` decoder and the sync consensus check. |
| [`src/keyRecord.ts`](src/keyRecord.ts)                                   | The key record layout (`177 + 97 · N + 65 · H` bytes) and its encryption.                                                                             |
| [`src/registry.ts`](src/registry.ts)                                     | The registered viewing key, current record and upload/apply builders.                                                                                 |
| [`src/storage.ts`](src/storage.ts)                                       | Buffer upload and apply instructions, account decoders and head PDA derivation.                                                                       |
| [`src/smartAccount.ts`](src/smartAccount.ts)                             | The Create, Register, Rotate and Spend flows.                                                                                                         |
| [`src/bytes.ts`](src/bytes.ts)                                           | The little-endian byte reader and writer.                                                                                                             |
| [`test/network/smartAccount.test.ts`](test/network/smartAccount.test.ts) | All flows end to end on devnet or mainnet.                                                                                                            |

## Key record encryption

Each ciphertext is `AES-256-CTR(key, iv = 0, plaintext)` with
`key = HKDF-SHA256(X25519(e, R), salt = E || R, info)`, where `e`/`E` is the
record's ephemeral keypair and `R` the recipient's tagged viewing public key.
The info is `KEY_INFO` for the shared viewing secret (to each signer),
`NULLIFIER_INFO` for the nullifier secret and `OLD_KEY_INFO || LE32(i)` for
historical viewing secret i. The latter two are encrypted to the current
shared viewing key; the per-index info gives each old secret its own
encryption key. Decryption checks each secret against its advertised public
key.

`oldViewingKeys` contains every old viewing key, oldest first, as a tagged
public key and an encrypted 32-byte secret. Each rotation appends the previous
current key and re-encrypts the full vector to the new shared viewing key.
Every current signer, including new members, receives the complete history.
The nullifier key stays unchanged. The storage program only stores these
serialized bytes; it does not parse this structure.

## Transactions

- The vault is the SPP owner signer and fee payer and pays the rent of its
  user registry record and storage head, all inside Squads CPIs, so it needs
  SOL. Squads strips the signer flag of its own members inside the CPI, so a
  member cannot pay there.
- Uploads are separate transactions. `prepareKeyRecordUpload` returns `store`
  instructions that append chunks of at most 1,024 bytes, growing the buffer
  as needed. Its rent payer signs uploads; the vault authorizes publication.
- Register sends the user registry `register` and storage `apply` for record 0
  in one Squads transaction. Rotate sends `update_keys` and `apply` of the new
  record, then the settings change in the same transaction. They run under the current signers and
  threshold. The deployed user registry takes only the viewing key in
  `update_keys`, so the client builds that instruction itself.
- Every Squads transaction is sync: `time_lock` is 0. The tests send version 1
  transactions (4096 bytes). Finalization carries the buffer address and its
  approved hash, so the whole record need not fit in that transaction.
- `createSmartAccount` returns key record 0, which the creator hands to the
  other signers for Register. It derives the Settings address from the Squads
  account counter, so if another account is created first the create fails
  and is built again.
- Rotate reads kept signers' viewing keys from their user registry records,
  so every kept signer must be registered.
- An approver calls `checkOwnEntry` before co-signing Register. For a rotate,
  pass the previous published record as the third argument, obtained from
  on-chain state. The client verifies every historical ciphertext and checks
  that the new vector preserves all previous keys in order.
- `apply` checks the complete upload, approved hash, predecessor and head
  revision. It seals the replacement, advances `[b"head", vault]` and its
  revision, closes the old record, and refunds its balance to the rent payer
  (the vault). These changes are atomic. Until apply succeeds, the old record
  stays active. The active record cannot be edited or separately closed.
  An unpublished buffer can be cancelled with its signer to refund rent.
- `recoverVaultKeys` decrypts the current and all old viewing keys directly
  from the latest record. It needs no historical record lookup. The SDK's
  `syncWallet` tries every key on the vault's outputs, so UTXOs sent before
  a member joined are visible too.

`rotateSmartAccount` returns `upload.instructions` and the final `instructions`.
Send each upload instruction in a separate transaction, have approvers check
the new record, then send the final instructions together. `registerVaultInstruction`
takes the address returned by `prepareKeyRecordUpload` as its `buffer` argument.
See [the storage protocol](program/STORAGE.md) for its layout and checks.
The replacement carries the entire encrypted key history. Only the current
record stays on chain after apply; its size is limited to 10 MiB including
the storage header. The storage program checks upload metadata and
authorization; key-record checks remain client-side.

## Run

Install Node.js 24+ and pnpm. Set the Helius RPC URLs in the environment;
the SDK uses the same endpoint's `/v1/spr` for the indexer and attested TEE
prover. The tests check the cluster's genesis hash, deployed programs,
Squads configuration, TEE attestation and proving keys before funding accounts.

```bash
pnpm install
pnpm test                    # unit tests
export SOLANA_DEVNET_RPC_URL='https://devnet.helius-rpc.com/?api-key=YOUR_KEY'
export SOLANA_MAINNET_RPC_URL='https://mainnet.helius-rpc.com/?api-key=YOUR_KEY'
pnpm test:devnet             # create, register, spend and rotate on devnet
pnpm test:mainnet            # the same flows on mainnet
```

Only the selected network's RPC variable is required. The payer defaults to
`~/.config/solana/id.json`; set `ZOLANA_PAYER_KEYPAIR` to use another file.
Fund it with at least 0.1 SOL on the selected network. The payer covers fees,
account rent and a 0.01 SOL private deposit. Cleanup returns unused public SOL
from the fresh members and vaults, including after a failed assertion. Current account
rent and the private balances remain on chain; superseded record rent is refunded.

CI runs the devnet suite using the existing `DEVNET_RPC_URL` and
`DEVNET_PAYER_KEYPAIR` repository secrets. Fork pull requests run the unit
checks only. Mainnet tests run locally.
