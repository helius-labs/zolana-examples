# DvP Node.js client

The examples use the program's generated WASM transactions and Xark prover, the
Zolana SDK for encryption and SPP proofs, and DvP instruction builders from
`src/instructions.ts`. Each example contains its full flow; utility functions
appear below `main`.

From `dvp/`, prepare the pinned source checkouts and generate the circuit
artifacts with their matching CLI:

```sh
scripts/setup-zk-tools.sh
export ZOLANA_CLI_BIN="$PWD/.tools/zolana/target/release/zolana"
export XARK_SOURCE="$PWD/.tools/xark"
"$ZOLANA_CLI_BIN" zk compile --test --manifest-path program/Cargo.toml --wasm-target nodejs --verifier
npm --prefix .tools/zolana ci
npm --prefix .tools/zolana run build:ts
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

The package uses the TypeScript SDK built from the same pinned Zolana
checkout as the Rust client. The circuit bytecode and Xark proving and verifying keys are checked
against `../dvp-keys.CHECKSUM` before loading. These deterministic keys are
for local tests.

## WASM proving

The generated Node module initializes when imported. Construct a prover for
one of the four compiled circuits:

```ts
import { ProgramProver } from "./src/wasm.js";

const prover = new ProgramProver("settle_dvp");
```

The wrapper uses Xark's synchronous proving session and verifies each proof
against the trusted key before converting it into the Solana proof layout.
Call `prover.free()` when finished to release the WASM session.
The generated artifacts live under `../target/zk/dvp-program/`, with
`<circuit>/circuit.xbc`, `<circuit>/pk.bin` and `<circuit>/vk.bin` for each
circuit and the combined Node module in `wasm/dvp_program.js`.

The [WASM benchmark](BENCHMARK.md) records Xark proof timings. Regenerate it
with `npx --no-install tsx bench/proofs.ts` from this directory.
