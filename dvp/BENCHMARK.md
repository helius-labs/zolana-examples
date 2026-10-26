# Private DvP -- CU Benchmark

1. Measure the five DvP instructions under Mollusk using the test release shielded-pool program. Each DvP handler verifies its generated program proof and invokes SPP directly. Cancellation and rejection share process_abort_dvp.

2. Record SPP and program proving times, consumed compute units, and serialized v1 transaction sizes. Create/reclaim use 1x2 with compact output padding; settle uses 3x4; cancel/reject use 3x2 with real optional deposits before random input padding.

3. Report compiled program constraints, checksummed proving-key image sizes, and the total bytes of verifying-key curve points. Release-profile program proof timings include loading the checked key image. Build the profiling program first with cargo build-sbf --manifest-path dvp/program/Cargo.toml --tools-version v1.56 --sbf-out-dir dvp/target/bench -- --features bpf-entrypoint,profile-program.

Regenerate with `cargo test --release --locked -p dvp-test --test bench_cu -- --ignored --nocapture`.

## Definitions

- **Total CU**: Compute units consumed by the function including all children
- **Net CU**: Compute units consumed by the function itself (excluding children)

## Table of Contents

1. [Cancel Dvp](#cancel-dvp)
2. [Circuits](#circuits)
3. [Create Dvp](#create-dvp)
4. [Reclaim Dvp](#reclaim-dvp)
5. [Reject Dvp](#reject-dvp)
6. [Settle Dvp](#settle-dvp)

## 1. Cancel Dvp

| Function                 |   Total CU |     Net CU |
| ------------------------ | ---------- | ---------- |
| `process_abort_dvp`      |    259,287 |    259,287 |
| `process_cancel_dvp_ix`  |    259,320 |         33 |

**Proving Time**
| SPP proof | DvP circuit proof | Total  |
| --------- | ----------------- | ------ |
|    188 ms |            285 ms | 474 ms |

**Transaction Size**
| Instruction Data | Accounts | v1 Tx      |
| ---------------- | -------- | ---------- |
|        904 bytes |       11 | 1417 bytes |

## 2. Circuits

| Circuit     | Constraints | Proving key | VK points | Proof time |
| ----------- | ----------- | ----------- | --------- | ---------- |
|  create_dvp |       11307 |     4.67 MB |     576 B |      87 ms |
| reclaim_dvp |       13211 |     5.28 MB |     576 B |     158 ms |
|  settle_dvp |       21313 |     8.92 MB |     576 B |     251 ms |
|   abort_dvp |       17071 |     7.56 MB |     576 B |     285 ms |

## 3. Create Dvp

| Function                 |   Total CU |     Net CU |
| ------------------------ | ---------- | ---------- |
| `process_create_dvp_ix`  |    242,932 |    242,932 |

**Proving Time**
| SPP proof | DvP circuit proof | Total  |
| --------- | ----------------- | ------ |
|     72 ms |             87 ms | 159 ms |

**Transaction Size**
| Instruction Data | Accounts | v1 Tx      |
| ---------------- | -------- | ---------- |
|       2835 bytes |        9 | 3282 bytes |

## 4. Reclaim Dvp

| Function                 |   Total CU |     Net CU |
| ------------------------ | ---------- | ---------- |
| `process_reclaim_dvp_ix` |    243,625 |    243,625 |

**Proving Time**
| SPP proof | DvP circuit proof | Total  |
| --------- | ----------------- | ------ |
|     75 ms |            158 ms | 233 ms |

**Transaction Size**
| Instruction Data | Accounts | v1 Tx      |
| ---------------- | -------- | ---------- |
|        681 bytes |        9 | 1128 bytes |

## 5. Reject Dvp

| Function                 |   Total CU |     Net CU |
| ------------------------ | ---------- | ---------- |
| `process_abort_dvp`      |    254,671 |    254,671 |
| `process_reject_dvp_ix`  |    254,704 |         33 |

**Proving Time**
| SPP proof | DvP circuit proof | Total  |
| --------- | ----------------- | ------ |
|    175 ms |            129 ms | 304 ms |

**Transaction Size**
| Instruction Data | Accounts | v1 Tx      |
| ---------------- | -------- | ---------- |
|        904 bytes |       11 | 1417 bytes |

## 6. Settle Dvp

| Function                 |   Total CU |     Net CU |
| ------------------------ | ---------- | ---------- |
| `process_settle_dvp_ix`  |    257,286 |    257,286 |

**Proving Time**
| SPP proof | DvP circuit proof | Total  |
| --------- | ----------------- | ------ |
|    181 ms |            251 ms | 432 ms |

**Transaction Size**
| Instruction Data | Accounts | v1 Tx      |
| ---------------- | -------- | ---------- |
|       1226 bytes |       11 | 1739 bytes |

