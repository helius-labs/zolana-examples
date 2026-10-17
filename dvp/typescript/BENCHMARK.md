# TypeScript WASM proof timings

Measured 2026-10-17T02:40:09.747Z at commit `cd6df749d1d1bfaf0b0f9a881a5a24fc6222e94c`.

darwin arm64; Apple M5 Pro; 18 logical CPUs; Node.js v26.0.0. The generated Node.js WASM build uses the release profile and one thread.

| Circuit     | Key read + SHA-256 (ms) | First fromKey (ms) | First prove (ms) | Warm prove median (ms) | Warm prove range (ms) |
| ----------- | ----------------------: | -----------------: | ---------------: | ---------------------: | --------------------: |
| create_dvp  |                     2.2 |              252.0 |            994.8 |                  912.7 |          773.0–1011.1 |
| reclaim_dvp |                     2.3 |              115.5 |            762.2 |                  763.5 |           753.3–765.9 |
| settle_dvp  |                     4.0 |              320.3 |           1522.5 |                 1371.1 |         1331.4–1399.4 |
| abort_dvp   |                     3.4 |              192.8 |           1162.3 |                 1075.6 |         1066.7–1096.1 |

Each warm figure uses five samples after one additional warm-up, with the same loaded prover and proof inputs. The fixtures cover create, a full delivery reclaim, a settlement with surplus, and cancellation with both deposits. Abort's first call uses an unfunded cancellation; its warm samples use both deposits.

`fromKey` includes WASM checksum validation, key decoding, and initial circuit preparation. `prove` includes proof generation and the prover's internal verification. Key read figures include the TypeScript loader's checksum check and use the warmed filesystem cache. Transaction construction, SDK encryption, WASM module loading, SPP proofs, and network requests are excluded. The existing circuit tests and public-input assertions must pass before this report is written.

Machine timings can vary with background load. Raw samples are in `../target/wasm-proof-benchmark.json`. WASM SHA-256: `444691e3db655ffb90afda874eab4a40ff1ac716835511fd34723a9dc81e3d3f`.

Reproduce from `dvp/typescript`:

```sh
npx --no-install tsx bench/proofs.ts
```
