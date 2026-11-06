# TypeScript WASM — 18 workers

Measured 2026-11-04T23:20:47.319Z at commit `c572fffecbe36dd3eef638c6e17c27ac15f103ce`.

darwin arm64; Apple M5 Pro; 18 logical CPUs; Node.js v26.0.0. The generated Node.js WASM build uses the release profile and Xark's streaming prover with a pool of 18 workers.

| Circuit     | Artifact read + SHA-256 (ms) | Prover load (ms) | First prove (ms) | Warm prove median (ms) | Warm prove range (ms) |
| ----------- | ---------------------------: | ---------------: | ---------------: | ---------------------: | --------------------: |
| create_dvp  |                          1.3 |             68.4 |            344.1 |                  256.4 |           251.2–263.6 |
| reclaim_dvp |                          1.2 |             61.3 |            323.4 |                  266.1 |           264.4–270.8 |
| settle_dvp  |                          2.0 |            104.4 |            506.2 |                  415.8 |           413.0–428.9 |
| abort_dvp   |                          1.7 |             80.7 |            434.6 |                  371.5 |           370.4–375.7 |

Each warm figure uses five samples after one additional warm-up, with the same loaded prover and proof inputs. The fixtures cover create, a full delivery reclaim, a settlement with surplus, and cancellation with both deposits. Abort's first call uses an unfunded cancellation; its warm samples use both deposits.

Prover load includes checked artifact reads, verifying key decoding, and Xark circuit and key preparation. `prove` includes proof generation and the prover's internal verification. Artifact read figures include the TypeScript loader's checksum check and use the warmed filesystem cache. Transaction construction, SDK encryption, WASM module loading, worker startup, SPP proofs, and network requests are excluded. The existing circuit tests and public-input assertions must pass before this report is written.

Combined WASM binary: 5.30 MiB (5,555,743 bytes); gzip -9: 1.78 MiB (1,871,520 bytes).

Worker-pool measurements are in [THREAD_BENCHMARK.md](THREAD_BENCHMARK.md).

Machine timings can vary with background load. Raw samples are in `../target/wasm-proof-benchmark.json`. WASM SHA-256: `0b73141b5c3b1907704ffa658e8e9c31611f006f1d26254ecf3aa6cca7dbb1ab`.

Reproduce from `dvp/typescript`:

```sh
npx --no-install tsx bench/proofs.ts
```

Set `DVP_WASM_THREADS` to choose the worker count, or `0` for the single-threaded session.
