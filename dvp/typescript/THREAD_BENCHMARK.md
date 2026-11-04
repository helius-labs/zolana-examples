# Node WASM worker comparison

Measured 2026-11-04T23:21:02.545Z at application commit `c572fffecbe36dd3eef638c6e17c27ac15f103ce` on Apple M5 Pro; Node.js v26.0.0.

Warm proof medians in milliseconds:

| Circuit     | Single session | 4 workers | 8 workers | 18 workers |
| ----------- | -------------: | --------: | --------: | ---------: |
| create_dvp  |          779.6 |     605.0 |     376.5 |      262.8 |
| reclaim_dvp |          819.7 |     634.6 |     395.6 |      313.3 |
| settle_dvp  |         1345.5 |     994.9 |     617.1 |      464.9 |
| abort_dvp   |         1196.5 |     898.3 |     572.6 |      377.0 |

Each configuration runs in a fresh process, with three samples after one warm-up and identical captured transaction inputs. The single session keeps a decoded key; the worker path uses Xark's streaming API with a shared worker pool and compressed key bytes. Circuit/key preparation, worker startup, transaction construction, encryption and SPP proofs are excluded. All proofs are verified, checked against the transaction's public hash, and checked for distinct randomness. Worker configurations must invoke both FFT and MSM kernels.

The fixtures cover create, delivery reclaim, settlement with surplus, and cancellation with both deposits funded. The normal circuit tests pass before measurements begin. The application's `ProgramProver` uses the single session by default and the worker path after `useWorkerPool(pool)`; `bench/proofs.ts` measures it with all available workers. Native Rust proves with Rayon.

WASM SHA-256: `0b73141b5c3b1907704ffa658e8e9c31611f006f1d26254ecf3aa6cca7dbb1ab`. Raw samples: `../target/wasm-thread-benchmark.json`.

Reproduce from `dvp/typescript`:

```sh
npx --no-install tsx bench/threads.ts
```
