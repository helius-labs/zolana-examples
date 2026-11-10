# TypeScript WASM — 18 workers

Measured 2026-11-08T18:27:52.109Z at commit `5eb427022144b7496fb1ae6777c9e3d50c5ecac1`.

darwin arm64; Apple M5 Pro; 18 logical CPUs; Node.js v26.0.0. The generated Node.js WASM build uses the release profile and Xark's streaming prover with a pool of 18 workers.

| Circuit     | Artifact read + SHA-256 (ms) | Prover load (ms) | First prove (ms) | Warm prove median (ms) | Warm prove range (ms) |
| ----------- | ---------------------------: | ---------------: | ---------------: | ---------------------: | --------------------: |
| create_dvp  |                          1.4 |             72.5 |            316.3 |                  255.2 |           253.5–269.0 |
| reclaim_dvp |                          1.4 |             63.5 |            287.6 |                  274.2 |           273.4–282.2 |
| settle_dvp  |                          2.1 |            109.7 |            495.8 |                  440.0 |           427.8–471.8 |
| abort_dvp   |                          1.8 |             78.4 |            422.9 |                  377.9 |           376.4–402.7 |

Each warm figure uses five samples after one additional warm-up, with the same loaded prover and proof inputs. The fixtures cover create, a full delivery reclaim, a settlement with surplus, and cancellation with both deposits. Abort's first call uses an unfunded cancellation; its warm samples use both deposits.

Prover load includes checked artifact reads, verifying key decoding, and Xark circuit and key preparation. `prove` includes proof generation and the prover's internal verification. Artifact read figures include the TypeScript loader's checksum check and use the warmed filesystem cache. Transaction construction, SDK encryption, WASM module loading, worker startup, SPP proofs, and network requests are excluded. The existing circuit tests and public-input assertions must pass before this report is written.

Combined WASM binary: 5.53 MiB (5,800,844 bytes); gzip -9: 1.87 MiB (1,956,371 bytes).

Worker-pool measurements are in [THREAD_BENCHMARK.md](THREAD_BENCHMARK.md).

Machine timings can vary with background load. Raw samples are in `../target/wasm-proof-benchmark.json`. WASM SHA-256: `52abe55ea5ff9f7c8506343ee6426867f6bf05833ebf6acf080b6223851c4f16`.

Reproduce from `dvp/typescript`:

```sh
npx --no-install tsx bench/proofs.ts
```

Set `DVP_WASM_THREADS` to choose the worker count, or `0` for the single-threaded session.
