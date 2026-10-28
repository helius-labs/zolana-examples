# TypeScript WASM — Xark session

Measured 2026-10-28T04:14:37.417Z at commit `c6717507fbb5da1187b465913a3b625182f9c1d0`.

darwin arm64; Apple M5 Pro; 18 logical CPUs; Node.js v26.0.0. The generated Node.js WASM build uses the release profile and a synchronous Xark proving session.

| Circuit     | Artifact read + SHA-256 (ms) | Prover load (ms) | First prove (ms) | Warm prove median (ms) | Warm prove range (ms) |
| ----------- | ---------------------------: | ---------------: | ---------------: | ---------------------: | --------------------: |
| create_dvp  |                          1.4 |           1306.3 |            910.1 |                  845.3 |           838.9–846.7 |
| reclaim_dvp |                          1.5 |           1777.9 |            913.5 |                  908.3 |           904.4–915.7 |
| settle_dvp  |                          2.4 |           2189.6 |           1301.4 |                 1221.6 |         1215.7–1225.2 |
| abort_dvp   |                          2.0 |           1937.8 |           1369.3 |                 1309.2 |         1305.9–1321.3 |

Each warm figure uses five samples after one additional warm-up, with the same loaded prover and proof inputs. The fixtures cover create, a full delivery reclaim, a settlement with surplus, and cancellation with both deposits. Abort's first call uses an unfunded cancellation; its warm samples use both deposits.

Prover load includes checked artifact reads, proving and verifying key decoding, and Xark circuit preparation. `prove` includes proof generation and the prover's internal verification. Artifact read figures include the TypeScript loader's checksum check and use the warmed filesystem cache. Transaction construction, SDK encryption, WASM module loading, SPP proofs, and network requests are excluded. The existing circuit tests and public-input assertions must pass before this report is written.

Machine timings can vary with background load. Raw samples are in `../target/wasm-proof-benchmark.json`. WASM SHA-256: `fed3b00f0d5c133c6a584360d401ea599c76285793a6ecda0c5d7e38fb7061a3`.

Reproduce from `dvp/typescript`:

```sh
npx --no-install tsx bench/proofs.ts
```
