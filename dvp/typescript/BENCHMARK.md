# TypeScript WASM proof timings

Measured 2026-10-17T03:00:12.070Z from source baseline `d72655851065bddec8283ac0d349abc422d77af0` with the Node threading changes applied.

darwin arm64; Apple M5 Pro; 18 logical CPUs; Node.js v26.0.0. Optimized WASM with a shared-memory Rayon pool.

Warm proof medians in milliseconds (five samples after one warm-up, reusing each prover):

| Circuit     | 1 worker | 4 workers | 8 workers | All available (18) |
| ----------- | -------: | --------: | --------: | -----------------: |
| create_dvp  |    791.9 |     386.3 |     255.3 |              206.9 |
| reclaim_dvp |    835.4 |     401.2 |     290.7 |              224.5 |
| settle_dvp  |   1495.3 |     600.6 |     442.2 |              338.7 |
| abort_dvp   |   1398.1 |     508.3 |     415.0 |              278.9 |

Each configuration ran sequentially in a fresh Node process using the same compiled WASM artifact and fixture scenarios. All existing circuit tests passed for every worker count, including cancel/reject funding combinations. The abort samples use cancellation with both deposits funded. Fixture blindings and Groth16 proof randomness vary between processes.

The measurement includes the prover’s internal proof verification. It excludes key loading, circuit preparation, WASM/worker initialization, transaction construction, SPP proofs, and network requests. Background applications were left running; treat small timing differences as noise.

# 1 thread

| Circuit     | Key read + SHA-256 (ms) | First fromKey (ms) | First prove (ms) | Warm prove median (ms) | Warm prove range (ms) |
| ----------- | ----------------------: | -----------------: | ---------------: | ---------------------: | --------------------: |
| create_dvp  |                     2.0 |              294.0 |            922.6 |                  791.9 |           785.0–872.6 |
| reclaim_dvp |                     2.1 |              118.0 |            874.7 |                  835.4 |           830.7–895.8 |
| settle_dvp  |                     3.8 |              258.9 |           1487.4 |                 1495.3 |         1483.2–1512.2 |
| abort_dvp   |                     3.0 |              181.5 |           1324.5 |                 1398.1 |         1361.9–1512.5 |

# 4 threads

| Circuit     | Key read + SHA-256 (ms) | First fromKey (ms) | First prove (ms) | Warm prove median (ms) | Warm prove range (ms) |
| ----------- | ----------------------: | -----------------: | ---------------: | ---------------------: | --------------------: |
| create_dvp  |                     1.9 |              273.5 |            412.5 |                  386.3 |           346.8–414.1 |
| reclaim_dvp |                     2.2 |              136.5 |            446.9 |                  401.2 |           376.9–448.1 |
| settle_dvp  |                     3.7 |              243.7 |            537.5 |                  600.6 |           526.1–612.7 |
| abort_dvp   |                     3.2 |              190.8 |            490.0 |                  508.3 |           471.0–585.8 |

# 8 threads

| Circuit     | Key read + SHA-256 (ms) | First fromKey (ms) | First prove (ms) | Warm prove median (ms) | Warm prove range (ms) |
| ----------- | ----------------------: | -----------------: | ---------------: | ---------------------: | --------------------: |
| create_dvp  |                     2.0 |              232.1 |            298.7 |                  255.3 |           248.5–273.9 |
| reclaim_dvp |                     2.2 |              131.4 |            281.4 |                  290.7 |           274.2–297.6 |
| settle_dvp  |                     3.7 |              271.5 |            454.5 |                  442.2 |           422.0–476.3 |
| abort_dvp   |                     3.1 |              201.0 |            394.8 |                  415.0 |           402.5–459.1 |

# All available cores — 18 threads

| Circuit     | Key read + SHA-256 (ms) | First fromKey (ms) | First prove (ms) | Warm prove median (ms) | Warm prove range (ms) |
| ----------- | ----------------------: | -----------------: | ---------------: | ---------------------: | --------------------: |
| create_dvp  |                     2.0 |              309.7 |            249.8 |                  206.9 |           201.9–209.0 |
| reclaim_dvp |                     2.3 |              129.5 |            221.6 |                  224.5 |           204.6–228.4 |
| settle_dvp  |                     3.8 |              302.6 |            366.9 |                  338.7 |           324.2–367.2 |
| abort_dvp   |                     3.2 |              198.8 |            306.2 |                  278.9 |           273.1–316.5 |

## Initialization and reproduction

Pool initialization accepts an explicit worker limit:

```ts
const wasm = await initWasm({ threads: 4 });
```

Omit `threads` to use Node’s `availableParallelism()`. Changing the limit after the module’s pool is initialized is rejected.

Reproduce each configuration from `dvp/typescript`:

```sh
DVP_WASM_THREADS=1 npx --no-install tsx bench/proofs.ts
DVP_WASM_THREADS=4 npx --no-install tsx bench/proofs.ts
DVP_WASM_THREADS=8 npx --no-install tsx bench/proofs.ts
DVP_WASM_THREADS=all npx --no-install tsx bench/proofs.ts
```

The harness also records checked key loading, first prover construction, and first proof timing. Matrix raw samples are in `../target/wasm-proof-benchmark-matrix.json`; individual samples are in `../target/wasm-proof-benchmark-{1,4,8,all}.json`. Each individual command rewrites this report with that configuration’s detailed measurements.

WASM SHA-256: `ac781e34612846a3e9ae8a2dbca1dd3304adf12e49db7ed6c76de593e022e842`.
