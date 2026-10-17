# DvP Node.js client

The examples use the program's generated WASM transactions and provers, the
Zolana SDK for encryption and SPP proofs, and DvP instruction builders from
`src/instructions.ts`. Each example contains its full flow; utility functions
appear below `main`.

From `dvp/`, generate the circuit artifacts with the pinned test CLI:

```sh
zolana zk compile --manifest-path program/Cargo.toml --wasm-target nodejs
cd typescript
npm ci
npm run check
npm test
```

Set the localnet endpoints and the three Ed25519 seeds (32 bytes in hex):

```sh
export ZOLANA_LOCALNET_URL=http://127.0.0.1:8899
export ZOLANA_INDEXER_URL=http://127.0.0.1:8784
export ZOLANA_PROVER_URL=http://127.0.0.1:3001
export DVP_SELLER_SEED=...
export DVP_BUYER_SEED=...
export DVP_AUTHORITY_SEED=...
export DVP_DELIVERY_MINT=...
export DVP_DELIVERY_ASSET_ID=...
```

The DvP program must be deployed at its declared address. The seller needs a
private balance in the delivery mint and public SOL for fees; the buyer needs
a private SOL balance. The Rust `node_client_lifecycles` test starts a localnet,
funds these balances, and runs all four examples.

```sh
npm run example -- examples/settle.ts
npm run example -- examples/reclaim.ts
npm run example -- examples/cancel.ts
npm run example -- examples/reject.ts
```

`settle.ts` creates the DvP, funds both legs, settles, and checks the payments
and surplus refunds. `reclaim.ts` creates and funds the delivery leg, then the
seller reclaims it. `cancel.ts` cancels an unfunded DvP with two real zero
refunds. `reject.ts` funds the payment leg, then the buyer rejects and receives
the payment refund.

The package pins a test SDK artifact attached to the Zolana test release for
this PR. The circuit keys are checked against `../dvp-keys.CHECKSUM` before
loading. These deterministic keys are for local tests.

## WASM threads

Initialize the generated Node module before creating a prover:

```ts
const wasm = await initWasm({ threads: 4 });
```

Omit `threads` to use all cores available to Node. The limit is set when the
module's pool initializes and cannot be changed afterward. Node builds use a
shared-memory Rayon pool; `--single-threaded` builds a package without it.

Use a CLI built from the Zolana source containing Node thread support. It emits
`node.mjs`, whose default initializer accepts `{ threads }`. The earlier PR test
release CLI predates that entry point.

The [WASM benchmark](BENCHMARK.md) records separate measurements for 1, 4, 8,
and all available threads.
