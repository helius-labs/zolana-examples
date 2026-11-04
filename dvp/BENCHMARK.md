# Private DvP -- CU Benchmark

1. Measure the five DvP instructions under Mollusk using the shielded-pool program built from Zolana 7c61c47a3 and the Xark-backed SDK. Each DvP handler verifies its generated program proof and invokes SPP directly. Cancellation and rejection share process_abort_dvp.

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
| `process_abort_dvp`      |    252,723 |    252,723 |
| `process_cancel_dvp_ix`  |    252,752 |         29 |

**Proving Time**
| SPP proof | DvP circuit proof | Total   |
| --------- | ----------------- | ------- |
|    152 ms |           1607 ms | 1759 ms |

**Transaction Size**
| Instruction Data | Accounts | v1 Tx      |
| ---------------- | -------- | ---------- |
|        904 bytes |       11 | 1417 bytes |

## 2. Circuits

| Circuit     | Constraints | Proving key | Verifying key | Proof time |
| ----------- | ----------- | ----------- | ------------- | ---------- |
|  create_dvp |       13645 |     2.70 MB |         296 B |    1115 ms |
| reclaim_dvp |       14676 |     2.86 MB |         296 B |    1192 ms |
|  settle_dvp |       23209 |     4.70 MB |         296 B |    1911 ms |
|   abort_dvp |       19522 |     4.14 MB |         296 B |    1607 ms |

## 3. Create Dvp

| Function                 |   Total CU |     Net CU |
| ------------------------ | ---------- | ---------- |
| `process_create_dvp_ix`  |    242,335 |    242,335 |

**Proving Time**
| SPP proof | DvP circuit proof | Total   |
| --------- | ----------------- | ------- |
|     54 ms |           1115 ms | 1170 ms |

**Transaction Size**
| Instruction Data | Accounts | v1 Tx      |
| ---------------- | -------- | ---------- |
|       2835 bytes |        9 | 3282 bytes |

## 4. Reclaim Dvp

| Function                 |   Total CU |     Net CU |
| ------------------------ | ---------- | ---------- |
| `process_reclaim_dvp_ix` |    245,674 |    245,674 |

**Proving Time**
| SPP proof | DvP circuit proof | Total   |
| --------- | ----------------- | ------- |
|     60 ms |           1192 ms | 1252 ms |

**Transaction Size**
| Instruction Data | Accounts | v1 Tx      |
| ---------------- | -------- | ---------- |
|        681 bytes |        9 | 1128 bytes |

## 5. Reject Dvp

| Function                 |   Total CU |     Net CU |
| ------------------------ | ---------- | ---------- |
| `process_abort_dvp`      |    255,077 |    255,077 |
| `process_reject_dvp_ix`  |    255,106 |         29 |

**Proving Time**
| SPP proof | DvP circuit proof | Total   |
| --------- | ----------------- | ------- |
|    141 ms |           1591 ms | 1733 ms |

**Transaction Size**
| Instruction Data | Accounts | v1 Tx      |
| ---------------- | -------- | ---------- |
|        904 bytes |       11 | 1417 bytes |

## 6. Settle Dvp

| Function                 |   Total CU |     Net CU |
| ------------------------ | ---------- | ---------- |
| `process_settle_dvp_ix`  |    256,799 |    256,799 |

**Proving Time**
| SPP proof | DvP circuit proof | Total   |
| --------- | ----------------- | ------- |
|    154 ms |           1911 ms | 2065 ms |

**Transaction Size**
| Instruction Data | Accounts | v1 Tx      |
| ---------------- | -------- | ---------- |
|       1226 bytes |       11 | 1739 bytes |

