# TypeScript WASM — Xark session

Measured 2026-10-28T13:44:06.942Z at commit `fbd589909e095066df0378be68993dbde86baed9`.

darwin arm64; Apple M5 Pro; 18 logical CPUs; Node.js v26.0.0. The generated Node.js WASM build uses the release profile and a synchronous Xark proving session.

| Circuit     | Artifact read + SHA-256 (ms) | Prover load (ms) | First prove (ms) | Warm prove median (ms) | Warm prove range (ms) |
| ----------- | ---------------------------: | ---------------: | ---------------: | ---------------------: | --------------------: |
| create_dvp  |                          1.4 |           1387.7 |            911.1 |                  843.0 |           842.4–848.5 |
| reclaim_dvp |                          1.5 |           1834.1 |            947.0 |                  912.9 |           909.5–917.5 |
| settle_dvp  |                          2.4 |           2200.3 |           1294.5 |                 1223.2 |         1221.6–1227.8 |
| abort_dvp   |                          2.4 |           1946.5 |           1374.0 |                 1309.0 |         1306.9–1314.1 |

Each warm figure uses five samples after one additional warm-up, with the same loaded prover and proof inputs. The fixtures cover create, a full delivery reclaim, a settlement with surplus, and cancellation with both deposits. Abort's first call uses an unfunded cancellation; its warm samples use both deposits.

Prover load includes checked artifact reads, proving and verifying key decoding, and Xark circuit preparation. `prove` includes proof generation and the prover's internal verification. Artifact read figures include the TypeScript loader's checksum check and use the warmed filesystem cache. Transaction construction, SDK encryption, WASM module loading, SPP proofs, and network requests are excluded. The existing circuit tests and public-input assertions must pass before this report is written.

Combined WASM binary: 5.09 MiB (5,338,165 bytes); gzip -9: 1.61 MiB (1,688,478 bytes).

Worker-pool measurements are in [THREAD_BENCHMARK.md](THREAD_BENCHMARK.md).

Machine timings can vary with background load. Raw samples are in `../target/wasm-proof-benchmark.json`. WASM SHA-256: `fed3b00f0d5c133c6a584360d401ea599c76285793a6ecda0c5d7e38fb7061a3`.

Reproduce from `dvp/typescript`:

```sh
npx --no-install tsx bench/proofs.ts
```
