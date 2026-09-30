# Zolana examples: Rust client

|  |  |
|---------|-------------|
| [`register_private_wallet`](examples/register_private_wallet.rs) | Register a private wallet. |
| [`deposit_transfer_withdraw`](examples/deposit_transfer_withdraw.rs) | Deposit, private transfer, and withdraw. Proves on the prover server, or on this machine with `--features local-prover`. |
| [`deposit_with_interface_setup`](examples/deposit_with_interface_setup.rs) | Create a token interface and deposit in one transaction. |
| [`sync_balance`](examples/sync_balance.rs) | Read the private SOL and SPL balances. |
| [`read_history`](examples/read_history.rs) | Read the private transaction history. |

## Setup

Copy the env template and set your [Helius API key](https://dashboard.helius.dev/):

```bash
cp .env.example .env
```

By default, the examples use your CLI wallet as `payer`. Make sure it's funded with [devnet SOL](https://faucet.solana.com/).

To run on localnet, toggle `localnet` in [`src/lib.rs`](src/lib.rs).

## Run

```bash
cargo run -p rust-client-example --example register_private_wallet
cargo run -p rust-client-example --example deposit_transfer_withdraw
cargo run -p rust-client-example --example deposit_with_interface_setup
cargo run -p rust-client-example --example sync_balance
cargo run -p rust-client-example --example read_history
```

`deposit_with_interface_setup` prepares a fresh test token, then checks that its interface PDA is absent. One transaction creates the mint registry PDA and token vault, then deposits into the sender's private balance. The sender pays transaction fees and account rent. The network must allow permissionless interface creation.

## Prove on this machine

With the `local-prover` feature, `deposit_transfer_withdraw` proves in process
with gnark ([`src/local_prover.rs`](src/local_prover.rs)) instead of sending the
proof request to the prover server. The request contains the nullifier secrets of
the spent notes, so they stay on this machine.

1. Install Go 1.27.1 or newer. The gnark prover is built from source.
2. Put the proving keys in `ZOLANA_PROVER_KEYS_DIR` (default
   `~/.config/zolana/proving-keys`). The zolana prover server downloads and
   verifies them there on first use, named as in the proving-key lockfile, for
   example `transfer_confidential_1_2.key`. Before it loads a key, the
   prover checks the file's sha256 against the one the on-chain verifying key
   pins, and refuses a key that does not match.
3. Run:

```bash
cargo run -p rust-client-example --example deposit_transfer_withdraw --features local-prover
```

## Documentation

- [Documentation](https://helius.dev/docs/privacy)
- [Source Code](https://github.com/helius-labs/zolana)
