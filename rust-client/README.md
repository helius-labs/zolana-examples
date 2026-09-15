# Zolana examples: Rust client

|  |  |
|---------|-------------|
| [`deposit_transfer_withdraw`](examples/deposit_transfer_withdraw.rs) | Deposit, private transfer, and withdraw. |
| [`register_wallet_with_merge`](examples/register_wallet_with_merge.rs) | Register a wallet, merge notes, and recover from a stale two-device merge. |

## Setup

Copy the env template and set your [Helius API key](https://dashboard.helius.dev/):

```bash
cp .env.example .env
```

By default, the examples use your CLI wallet as `payer`. Make sure it's funded with [devnet SOL](https://faucet.solana.com/).

To run on localnet, toggle `localnet` in [`src/lib.rs`](src/lib.rs).

## Run

```bash
cargo run -p rust-client-example --example deposit_transfer_withdraw
cargo run -p rust-client-example --example register_wallet_with_merge
```

### What `register_wallet_with_merge` shows

1. Registers the wallet and enables merging in one transaction.
2. Deposits three `0.1 SOL` notes and syncs two devices to the same wallet state.
3. Device A merges two notes, leaving the wallet with two notes and the same `0.3 SOL` balance.
4. Device B submits a merge built from its stale state. The request is rejected because Device A already spent the shared input nullifiers.
5. Device B syncs from Device A's confirmed slot, recreates the merge, and submits it with fresh Merkle proofs and current `root_index` values.
6. The retry succeeds, leaving the original `0.3 SOL` balance consolidated into one note.

The example runs against devnet and asserts each balance and note-count transition.

## Documentation

- [Documentation](https://helius.dev/docs/privacy)
- [Source Code](https://github.com/helius-labs/zolana)
