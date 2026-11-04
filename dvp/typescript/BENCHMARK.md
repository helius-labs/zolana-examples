# TypeScript WASM — 18 workers

Measured 2026-11-04T03:54:40.707Z at commit `51c4a9a6072e662167e9f74e8ebee42eea8237b6`.

darwin arm64; Apple M5 Pro; 18 logical CPUs; Node.js v26.0.0. The generated Node.js WASM build uses the release profile and Xark's streaming prover with a pool of 18 workers.

| Circuit     | Artifact read + SHA-256 (ms) | Prover load (ms) | First prove (ms) | Warm prove median (ms) | Warm prove range (ms) |
| ----------- | ---------------------------: | ---------------: | ---------------: | ---------------------: | --------------------: |
| create_dvp  |                          1.3 |             76.4 |            358.6 |                  261.6 |           256.4–263.6 |
| reclaim_dvp |                          1.3 |             68.5 |            321.2 |                  287.8 |           282.3–302.0 |
| settle_dvp  |                          2.2 |            111.6 |            505.2 |                  432.4 |           427.3–438.7 |
| abort_dvp   |                          2.1 |             85.1 |            456.6 |                  398.9 |           395.7–424.6 |

Each warm figure uses five samples after one additional warm-up, with the same loaded prover and proof inputs. The fixtures cover create, a full delivery reclaim, a settlement with surplus, and cancellation with both deposits. Abort's first call uses an unfunded cancellation; its warm samples use both deposits.

Prover load includes checked artifact reads, verifying key decoding, and Xark circuit and key preparation. `prove` includes proof generation and the prover's internal verification. Artifact read figures include the TypeScript loader's checksum check and use the warmed filesystem cache. Transaction construction, SDK encryption, WASM module loading, worker startup, SPP proofs, and network requests are excluded. The existing circuit tests and public-input assertions must pass before this report is written.

Combined WASM binary: 5.10 MiB (5,346,322 bytes); gzip -9: 1.62 MiB (1,694,735 bytes).

Worker-pool measurements are in [THREAD_BENCHMARK.md](THREAD_BENCHMARK.md).

Machine timings can vary with background load. Raw samples are in `../target/wasm-proof-benchmark.json`. WASM SHA-256: `47f58127c561e478fcc8c83fbd061cdb698ee97209d81dd163dfcb0d59e94b03`.

Reproduce from `dvp/typescript`:

```sh
npx --no-install tsx bench/proofs.ts
```

Set `DVP_WASM_THREADS` to choose the worker count, or `0` for the single-threaded session.
