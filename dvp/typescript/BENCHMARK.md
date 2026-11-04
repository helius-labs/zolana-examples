# TypeScript WASM — 18 workers

Measured 2026-11-04T03:45:08.837Z at commit `3597be32b77bc480a0eaa8289ae3dc126e7f982f`.

darwin arm64; Apple M5 Pro; 18 logical CPUs; Node.js v26.0.0. The generated Node.js WASM build uses the release profile and Xark's streaming prover with a pool of 18 workers.

| Circuit     | Artifact read + SHA-256 (ms) | Prover load (ms) | First prove (ms) | Warm prove median (ms) | Warm prove range (ms) |
| ----------- | ---------------------------: | ---------------: | ---------------: | ---------------------: | --------------------: |
| create_dvp  |                          1.3 |             74.4 |            366.0 |                  262.7 |           259.1–266.7 |
| reclaim_dvp |                          1.4 |             69.7 |            328.9 |                  289.7 |           280.4–293.0 |
| settle_dvp  |                          2.1 |            112.7 |            505.4 |                  430.9 |           425.6–451.2 |
| abort_dvp   |                          1.9 |             87.6 |            464.6 |                  398.7 |           388.1–419.4 |

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
