# Private DvP -- CU Benchmark

1. Measure the five DvP instructions under Mollusk using the v0.5.0-alpha shielded-pool program and the Xark-backed SDK. Each DvP handler verifies its generated program proof and invokes SPP directly. Cancellation and rejection share process_abort_dvp.

2. Record SPP and program proving times, consumed compute units, and serialized v1 transaction sizes. Create/reclaim use 1x2 with compact output padding; settle uses 3x4; cancel/reject use 3x2 with real optional deposits before random input padding.

3. Report compiled program constraints, checksummed Xark proving-key and verifying-key sizes. Native proof times are single samples in the release profile, including artifact checksum checks, circuit/key loading, proving and verification. Build the profiling program first with cargo build-sbf --manifest-path dvp/program/Cargo.toml --tools-version v1.54 --sbf-out-dir dvp/target/bench -- --features bpf-entrypoint,profile-program.

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
| `process_abort_dvp`      |    254,090 |    254,090 |
| `process_cancel_dvp_ix`  |    254,119 |         29 |

**Proving Time**
| SPP proof | DvP circuit proof | Total   |
| --------- | ----------------- | ------- |
|    144 ms |           1758 ms | 1902 ms |

**Transaction Size**
| Instruction Data | Accounts | v1 Tx      |
| ---------------- | -------- | ---------- |
|        904 bytes |       11 | 1417 bytes |

## 2. Circuits

| Circuit     | Constraints | Proving key | Verifying key | Proof time |
| ----------- | ----------- | ----------- | ------------- | ---------- |
|  create_dvp |       14127 |     2.78 MB |         296 B |    1195 ms |
| reclaim_dvp |       15455 |     2.98 MB |         296 B |    1232 ms |
|  settle_dvp |       24549 |     4.91 MB |         296 B |    2021 ms |
|   abort_dvp |       20864 |     4.35 MB |         296 B |    1758 ms |

## 3. Create Dvp

| Function                 |   Total CU |     Net CU |
| ------------------------ | ---------- | ---------- |
| `process_create_dvp_ix`  |    242,216 |    242,216 |

**Proving Time**
| SPP proof | DvP circuit proof | Total   |
| --------- | ----------------- | ------- |
|     58 ms |           1195 ms | 1254 ms |

**Transaction Size**
| Instruction Data | Accounts | v1 Tx      |
| ---------------- | -------- | ---------- |
|       2835 bytes |        9 | 3282 bytes |

## 4. Reclaim Dvp

| Function                 |   Total CU |     Net CU |
| ------------------------ | ---------- | ---------- |
| `process_reclaim_dvp_ix` |    245,554 |    245,554 |

**Proving Time**
| SPP proof | DvP circuit proof | Total   |
| --------- | ----------------- | ------- |
|     61 ms |           1232 ms | 1294 ms |

**Transaction Size**
| Instruction Data | Accounts | v1 Tx      |
| ---------------- | -------- | ---------- |
|        681 bytes |        9 | 1128 bytes |

## 5. Reject Dvp

| Function                 |   Total CU |     Net CU |
| ------------------------ | ---------- | ---------- |
| `process_abort_dvp`      |    254,574 |    254,574 |
| `process_reject_dvp_ix`  |    254,603 |         29 |

**Proving Time**
| SPP proof | DvP circuit proof | Total   |
| --------- | ----------------- | ------- |
|    150 ms |           1708 ms | 1858 ms |

**Transaction Size**
| Instruction Data | Accounts | v1 Tx      |
| ---------------- | -------- | ---------- |
|        904 bytes |       11 | 1417 bytes |

## 6. Settle Dvp

| Function                 |   Total CU |     Net CU |
| ------------------------ | ---------- | ---------- |
| `process_settle_dvp_ix`  |    258,159 |    258,159 |

**Proving Time**
| SPP proof | DvP circuit proof | Total   |
| --------- | ----------------- | ------- |
|    149 ms |           2021 ms | 2170 ms |

**Transaction Size**
| Instruction Data | Accounts | v1 Tx      |
| ---------------- | -------- | ---------- |
|       1226 bytes |       11 | 1739 bytes |

