# Examples for Solana Privacy Rings

> [!WARNING]
> **Not for production.** These are examples, not audited code. The escrow and
> swap programs prove with insecure test keys generated from a fixed public
> seed, so anyone can forge their proofs, and their program IDs are example IDs.
> Do not deploy the programs or use them with real funds. The client examples
> show how to call the SDK; review and adapt them before using them in an
> application.

### [Rust client](rust-client/README.md)

|  |  |
|---------|-------------|
| [`register_private_wallet`](rust-client/examples/register_private_wallet.rs) | Register a private wallet. |
| [`deposit_transfer_withdraw`](rust-client/examples/deposit_transfer_withdraw.rs) | Deposit, private transfer, and withdraw. |
| [`deposit_with_interface_setup`](rust-client/examples/deposit_with_interface_setup.rs) | Create a token interface and deposit in one transaction. |
| [`sync_balance`](rust-client/examples/sync_balance.rs) | Read the private SOL and SPL balances. |
| [`read_history`](rust-client/examples/read_history.rs) | Read the private transaction history. |

### [TypeScript client](typescript-client/README.md)

|  |  |
|---------|-------------|
| [`register_private_wallet`](typescript-client/examples/register_private_wallet.ts) | Register a private wallet. |
| [`deposit_transfer_withdraw`](typescript-client/examples/deposit_transfer_withdraw.ts) | Deposit, private transfer, and withdraw. |
| [`deposit_with_interface_setup`](typescript-client/examples/deposit_with_interface_setup.ts) | Create a token interface and deposit in one transaction. |
| [`sync_balance`](typescript-client/examples/sync_balance.ts) | Read the private SOL and SPL balances. |
| [`read_history`](typescript-client/examples/read_history.ts) | Read the private transaction history. |

### Program examples

|  |  |
|---------|-------------|
| [`swap/`](swap/) | A confidential swap between a maker and a taker. |
| [`escrow/`](escrow/) | A timelock escrow on SPP: lock a private balance until a deadline, then release or reclaim. |

### [Private smart account (client only)](smart-account-client/README.md)

|  |  |
|---------|-------------|
| [`smart-account-client/`](smart-account-client/) | A Squads smart account whose vault holds a private balance, with a shared viewing key encrypted to every signer off chain. TypeScript, no program. |

## Documentation

CI runs all five Rust and TypeScript client examples and the swap (take and
cancel) and timelock escrow flows of both implementations: the gnark ones in
the `examples` workflow and the Xark ones in `swap-xark` and `escrow-xark`.
All Rust examples and the Rust client pin the Zolana crates as Git
dependencies at one commit, `ca4cf4f6f0c83c0b225342e4bc6f16b3da83eb08`: the
[`cli-v0.10.0`](https://github.com/helius-labs/zolana/releases/tag/cli-v0.10.0)
release commit `357dd92` plus the fetchable gnark-sdk Go module, with the same
SDK. No crates.io release matches the deployed 1.0 programs yet. The Xark
implementations also pin Xark 0.3.0; see [`swap/README.md`](swap/README.md)
and [`escrow/README.md`](escrow/README.md).
The TypeScript client uses the npm release `@heliuslabs/spr@0.9.0-alpha`,
which has the paged indexer API of the `cli-v0.10.0` Photon. It predates the
deployed 1.0 programs' per-tree fee defaults and user-registry `update_keys`
data, which the examples do not use; a matching npm release replaces it.
The gnark circuits fetch the Go modules
`github.com/helius-labs/zolana/sdk-libs/gnark-sdk` and `prover/server` at the
same commit. The integration tests run against the `cli-v0.10.0` release: its
CLI is checked against the release's `SHA256SUMS`, and `zolana dev artifacts`
downloads the prover, Photon and the deployed `shielded-pool-v1.0.0` program
it pins and checks their SHA-256, and downloads surfpool and checks its
version. The deployed `user-registry-v1.0.0` program is pinned by SHA-256 in
[`test-utils/zolana-release.sh`](test-utils/zolana-release.sh).
Nothing clones or builds Zolana.
The shielded-pool program ID is
`ring1GaLCuJnkUMVkX2JeZLTTX17wKsV4fZmPVzXANW`; accounts and notes from
earlier program IDs are not migrated.

To run the same tests locally on darwin-arm64 or linux-x64, install Rust,
Go 1.27.3+, and the Solana CLI with `cargo build-sbf`. Install the TypeScript
dependencies with `cd typescript-client && pnpm install --frozen-lockfile`,
then run from the repository root:

```bash
bash test-utils/run-release-tests.sh
```

The runner installs the release CLI and artifacts, builds the two example
programs, generates the examples' insecure test proving keys, and starts
isolated validators and indexers. The protocol prover downloads its pinned keys
as needed. `ZOLANA_PORT_OFFSET` and `ZOLANA_PROVER_URL` move its ports.
Logs are retained under `target/release-tests/processes/logs` and uploaded by CI.

The separate `devnet` workflow runs both client suites on Solana devnet, using
one Helius RPC endpoint for Solana RPC, the Photon indexer, and the attested TEE
prover. Indexer and prover requests use `/v1/spr` on the same host with the same
API key. It runs on pushes to main, pull requests
from this repository, and manual dispatch. Configure `DEVNET_RPC_URL` and
`DEVNET_PAYER_KEYPAIR` as repository secrets, with a devnet-only JSON keypair
funded with at least 0.2 SOL. Each client uses a fresh funded payer, and unused
public SOL is returned to the CI wallet. The test checks the devnet genesis hash,
deployed programs, TEE attestation, and proving keys before funding the test payers.

Both clients default to devnet and verify its signed TEE policy. Set
`ZOLANA_ENDPOINT=https://mainnet.helius-rpc.com/?api-key=YOUR_KEY` to use mainnet
and its signed TEE policy instead. Local stacks can override the RPC, indexer,
and prover URLs separately using the client `.env.example` files.

- [Demo](https://helius-privacy-demo.fly.dev/)
- [Documentation](https://helius.dev/docs/privacy)
- [Source Code](https://github.com/helius-labs/zolana)
