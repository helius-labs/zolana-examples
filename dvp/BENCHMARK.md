# Private DvP -- CU Benchmark

1. Measure the five DvP instructions under Mollusk using the shielded-pool program built from Zolana 3541afcc5 and the Xark-backed SDK. Each DvP handler verifies its generated program proof and invokes SPP directly. Cancellation and rejection share process_abort_dvp.

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
| `process_abort_dvp`      |    254,838 |    254,838 |
| `process_cancel_dvp_ix`  |    254,867 |         29 |

**Proving Time**
| SPP proof | DvP circuit proof | Total   |
| --------- | ----------------- | ------- |
|    157 ms |           1663 ms | 1821 ms |

**Transaction Size**
| Instruction Data | Accounts | v1 Tx      |
| ---------------- | -------- | ---------- |
|        904 bytes |       11 | 1417 bytes |

## 2. Circuits

| Circuit     | Constraints | Proving key | Verifying key | Proof time |
| ----------- | ----------- | ----------- | ------------- | ---------- |
|  create_dvp |       13645 |     2.70 MB |         296 B |    1113 ms |
| reclaim_dvp |       14676 |     2.86 MB |         296 B |    1155 ms |
|  settle_dvp |       23209 |     4.70 MB |         296 B |    2022 ms |
|   abort_dvp |       19522 |     4.14 MB |         296 B |    1663 ms |

## 3. Create Dvp

| Function                 |   Total CU |     Net CU |
| ------------------------ | ---------- | ---------- |
| `process_create_dvp_ix`  |    244,434 |    244,434 |

**Proving Time**
| SPP proof | DvP circuit proof | Total   |
| --------- | ----------------- | ------- |
|     59 ms |           1113 ms | 1172 ms |

**Transaction Size**
| Instruction Data | Accounts | v1 Tx      |
| ---------------- | -------- | ---------- |
|       2835 bytes |        9 | 3282 bytes |

## 4. Reclaim Dvp

| Function                 |   Total CU |     Net CU |
| ------------------------ | ---------- | ---------- |
| `process_reclaim_dvp_ix` |    245,557 |    245,557 |

**Proving Time**
| SPP proof | DvP circuit proof | Total   |
| --------- | ----------------- | ------- |
|     59 ms |           1155 ms | 1215 ms |

**Transaction Size**
| Instruction Data | Accounts | v1 Tx      |
| ---------------- | -------- | ---------- |
|        681 bytes |        9 | 1128 bytes |

## 5. Reject Dvp

| Function                 |   Total CU |     Net CU |
| ------------------------ | ---------- | ---------- |
| `process_abort_dvp`      |    255,735 |    255,735 |
| `process_reject_dvp_ix`  |    255,764 |         29 |

**Proving Time**
| SPP proof | DvP circuit proof | Total   |
| --------- | ----------------- | ------- |
|    148 ms |           1586 ms | 1734 ms |

**Transaction Size**
| Instruction Data | Accounts | v1 Tx      |
| ---------------- | -------- | ---------- |
|        904 bytes |       11 | 1417 bytes |

## 6. Settle Dvp

| Function                 |   Total CU |     Net CU |
| ------------------------ | ---------- | ---------- |
| `process_settle_dvp_ix`  |    258,181 |    258,181 |

**Proving Time**
| SPP proof | DvP circuit proof | Total   |
| --------- | ----------------- | ------- |
|    158 ms |           2022 ms | 2180 ms |

**Transaction Size**
| Instruction Data | Accounts | v1 Tx      |
| ---------------- | -------- | ---------- |
|       1226 bytes |       11 | 1739 bytes |

