# Rust Client SDK Examples

End-to-end client examples for private balances and transactions, driven through
the `zolana-client` SDK against a devnet deployment (RPC, Photon indexer, and
prover). Each example is a self-contained binary covering one operation: it opens
by building its own connection (RPC, indexer, prover, payer, tree) so you see how
to reach the deployment, then holds the SDK call it demonstrates. The helpers in
`src/lib.rs` cover only the seeding a real app never hand-writes (register a
throwaway mint, fund a fee key, create a private wallet, deposit).

Each example moves an SPL value; a comment in each shows the SOL variant.

## Examples

- **`create_private_wallet`** (proofless): create the keypair, fund a Solana fee
  key, build the wallet, and register it so others can send to it privately.
- **`deposit`** (proofless): deposit a public balance into a private balance with
  `create_deposit`, sent at the instruction level.
- **`transfer`** (private send): move a value privately between two private
  balances, spending one note and one SOL fee note. Proven.
- **`withdraw`**: withdraw a value back to a public account (an ATA for SPL, a
  wallet for SOL), building the withdrawal target by hand. Proven.
- **`sync_balance`**: query the indexer for a wallet's encrypted UTXOs by view
  tag, the raw layer under `sync_wallet`.

Transfer and withdraw build the transaction by hand: select inputs, add the
output, sign, then let the one-call `Submit` action fetch the merkle and
non-inclusion proofs, assemble the witness, prove, and send the `Transact`
instruction.

The wallet owns its `AssetRegistry`, so the SDK reads asset ids off the wallet
(`sync_wallet`, `get_private_token_balances`, and the transaction builder take no
separate registry argument). Register SPL assets before creating the parties that
spend them.

## Configure

The devnet URLs are literals in each example: RPC
`https://devnet.helius-rpc.com/?api-key={API_KEY}`, indexer
`http://202.8.10.77:8784/`, prover `http://202.8.10.77:3011/`. Three values come
from the environment:

| Variable | Meaning | Default |
|----------|---------|---------|
| `API_KEY` | Helius key, injected into the RPC URL | required |
| `ZOLANA_TREE` | the deployment's state tree address | required |
| `ZOLANA_PAYER_KEYPAIR` | fee payer keypair file | `~/.config/solana/id.json` |

The payer must already hold SOL; there is no airdrop. There is no tree discovery,
so `ZOLANA_TREE` is required (current devnet:
`treeYbr45LjxovKvtD46uEphM64kwoFFPYhVNw1A8x8`).

Transfers and withdrawals generate a proof; on first use the prover downloads its
proving keys from a GitHub release, which needs `gh` authenticated for the hosting
org (`gh auth status`). Deposits and sync are proofless and need neither.

## Run

```bash
API_KEY=<helius key> ZOLANA_TREE=<tree> ZOLANA_PAYER_KEYPAIR=<funded key> \
  cargo run -p rust-client-example --example deposit
```

Start with `deposit` or `sync_balance`: they are proofless, so they validate the
connection without the prover or `gh`. Each example prints a single `ok ...` line
with the transaction signature and the resulting balance.

Registering an SPL asset needs a program that allows permissionless SPL interface
creation. `create_private_wallet` registers the wallet address only where the
user-registry program is deployed; where it is not, registration is skipped with
a note and deposits and reads still work.

## Documentation

- Protocol spec: [`docs/spec.md`](../../zolana-devx/docs/spec.md)
- Client SDK: [`sdk-libs/client`](../../zolana-devx/sdk-libs/client)
