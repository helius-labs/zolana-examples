# Node WASM worker comparison

Measured 2026-11-04T03:49:40.533Z at application commit `3597be32b77bc480a0eaa8289ae3dc126e7f982f` on Apple M5 Pro; Node.js v26.0.0.

Warm proof medians in milliseconds:

| Circuit     | Single session | 4 workers | 8 workers | 18 workers |
| ----------- | -------------: | --------: | --------: | ---------: |
| create_dvp  |          799.5 |     628.2 |     409.4 |      267.2 |
| reclaim_dvp |          867.0 |     671.4 |     429.6 |      282.3 |
| settle_dvp  |         1203.9 |    1049.6 |     674.2 |      436.2 |
| abort_dvp   |         1300.2 |     932.2 |     569.7 |      399.3 |

Each configuration runs in a fresh process, with three samples after one warm-up and identical captured transaction inputs. The single session keeps a decoded key; the worker path uses Xark's streaming API with a shared worker pool and compressed key bytes. Circuit/key preparation, worker startup, transaction construction, encryption and SPP proofs are excluded. All proofs are verified, checked against the transaction's public hash, and checked for distinct randomness. Worker configurations must invoke both FFT and MSM kernels.

The fixtures cover create, delivery reclaim, settlement with surplus, and cancellation with both deposits funded. The normal circuit tests pass before measurements begin. These timings show the available threaded API; the application's `ProgramProver` still defaults to a single session. Native Rust already uses Rayon.

WASM SHA-256: `47f58127c561e478fcc8c83fbd061cdb698ee97209d81dd163dfcb0d59e94b03`. Raw samples: `../target/wasm-thread-benchmark.json`.

Reproduce from `dvp/typescript`:

```sh
npx --no-install tsx bench/threads.ts
```
