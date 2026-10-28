# Node WASM worker comparison

Measured 2026-10-28T13:40:35.257Z at application commit `fbd589909e095066df0378be68993dbde86baed9` on Apple M5 Pro; Node.js v26.0.0.

Warm proof medians in milliseconds:

| Circuit     | Single session | 4 workers | 8 workers | 18 workers |
| ----------- | -------------: | --------: | --------: | ---------: |
| create_dvp  |          851.0 |     663.3 |     416.6 |      285.1 |
| reclaim_dvp |          910.3 |     700.4 |     451.0 |      325.3 |
| settle_dvp  |         1231.5 |    1087.6 |     669.2 |      460.0 |
| abort_dvp   |         1320.7 |     968.8 |     600.9 |      437.0 |

Each configuration runs in a fresh process, with three samples after one warm-up and identical captured transaction inputs. The single session keeps a decoded key; the worker path uses Xark's streaming API with a shared worker pool and compressed key bytes. Circuit/key preparation, worker startup, transaction construction, encryption and SPP proofs are excluded. All proofs are verified, checked against the transaction's public hash, and checked for distinct randomness. Worker configurations must invoke both FFT and MSM kernels.

The fixtures cover create, delivery reclaim, settlement with surplus, and cancellation with both deposits funded. The normal circuit tests pass before measurements begin. These timings show the available threaded API; the application's `ProgramProver` still defaults to a single session. Native Rust already uses Rayon.

WASM SHA-256: `fed3b00f0d5c133c6a584360d401ea599c76285793a6ecda0c5d7e38fb7061a3`. Raw samples: `../target/wasm-thread-benchmark.json`.

Reproduce from `dvp/typescript`:

```sh
npx --no-install tsx bench/threads.ts
```
