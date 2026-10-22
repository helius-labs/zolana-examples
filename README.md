# Examples for private Solana rings

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
| [`swap-program/`](swap-program/) | A confidential swap between a maker and a taker. |
| [`escrow-program/`](escrow-program/) | A timelock escrow on SPP: lock a private balance until a deadline, then release or reclaim. |

## Documentation

CI runs all five Rust and TypeScript client examples, the swap (take, cancel,
and verifiable encryption), and timelock escrow flows against the
`v0.5.0-alpha` GitHub release binaries, including the shielded
pool, user registry, prover, and Photon indexer. Artifact sizes and SHA-256 hashes
are pinned in [`test-utils/release-artifacts.json`](test-utils/release-artifacts.json).

To run the same tests locally, install Rust, Go 1.27.1+, and the Solana CLI
with `cargo build-sbf`, and check out Zolana's `v0.5.0-alpha` tag in a sibling
directory named `zolana`. Install the TypeScript dependencies with
`cd typescript-client && pnpm install --frozen-lockfile`, then run from the
repository root:

```bash
bash test-utils/run-release-tests.sh
```

The runner downloads and verifies release binaries, builds the two example
programs, generates their insecure test proving keys, and starts isolated
validators and indexers. The protocol prover downloads its pinned keys as needed.
Logs are retained under `target/release-tests/processes/logs` and uploaded by CI.

The separate `devnet` workflow runs both client suites on Solana devnet, using
Photon at `https://d21d98xh5lgv3b.cloudfront.net` and the attested prover at
`https://d1unet9hn0mci0.cloudfront.net`. It runs on pushes to main, pull requests
from this repository, and manual dispatch. Configure `DEVNET_RPC_URL` and
`DEVNET_PAYER_KEYPAIR` as repository secrets, with a devnet-only JSON keypair
funded with at least 0.2 SOL. Each client uses a fresh funded payer, and unused
public SOL is returned to the CI wallet. The test checks the devnet genesis hash,
deployed programs, and TEE attestation before funding the test payers.

- [Demo](https://helius-privacy-demo.fly.dev/)
- [Documentation](https://helius.dev/docs/privacy)
- [Source Code](https://github.com/helius-labs/zolana)
