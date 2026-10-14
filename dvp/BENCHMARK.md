# Private DvP -- CU Benchmark

Compute unit profiling for the private DvP create_dvp/reclaim_dvp/settle_dvp/cancel_dvp/reject_dvp instructions, replayed under mollusk. The shielded-pool tree account is built directly (the program's `create_tree` init plus the input utxo hashes appended), and each instruction verifies its own Groth16 proof, then CPIs SPP `transact` (the `cpi_spp_transact*` row). Only the DvP program is profiled; the shielded-pool program is the plain v0.4.0-alpha release build, so the CU its CPI consumes is charged to the `cpi_spp_transact*` row as a black box and its internal functions do not appear here. The SPP shapes are 1x2 for create_dvp (the address slot in, the DvP UTXO and compact padding out) and reclaim_dvp (a deposit in, the refund and compact padding out), 3x4 for settle_dvp, and 3x2 for cancel_dvp (both deposits) and reject_dvp (the payment deposit only, so one padding input). Each instruction section also records its proving times (SPP proof plus the DvP circuit proof) and the serialized size of the version 1 transaction it is sent as, which states its compute ceilings in the message header and may run to 4096 bytes. The profiled program must be built first: `cargo build-sbf --manifest-path dvp/program/Cargo.toml --sbf-out-dir dvp/target/bench -- --features bpf-entrypoint,profile-program`.

Regenerate with `cargo test -p dvp-test --test bench_cu -- --ignored --nocapture`.

## Definitions

- **Total CU**: Compute units consumed by the function including all children
- **Net CU**: Compute units consumed by the function itself (excluding children)

## Table of Contents

1. [Cancel Dvp](#cancel-dvp)
2. [Create Dvp](#create-dvp)
3. [Reclaim Dvp](#reclaim-dvp)
4. [Reject Dvp](#reject-dvp)
5. [Settle Dvp](#settle-dvp)

## 1. Cancel Dvp

| Function                  |   Total CU |     Net CU |
| ------------------------- | ---------- | ---------- |
| `cpi_spp_transact_signed` |    151,150 |    151,150 |
| `process_cancel_dvp_ix`   |    255,248 |    104,098 |

**Proving Time**
| SPP proof | DvP circuit proof | Total  |
| --------- | ----------------- | ------ |
|    234 ms |             29 ms | 264 ms |

**Transaction Size**
| Instruction Data | Accounts | v1 Tx      |
| ---------------- | -------- | ---------- |
|        904 bytes |       11 | 1417 bytes |

## 2. Create Dvp

| Function                  |   Total CU |     Net CU |
| ------------------------- | ---------- | ---------- |
| `cpi_spp_transact_signed` |    140,436 |    140,436 |
| `process_create_dvp_ix`   |    244,151 |    103,715 |

**Proving Time**
| SPP proof | DvP circuit proof | Total  |
| --------- | ----------------- | ------ |
|     64 ms |             82 ms | 146 ms |

**Transaction Size**
| Instruction Data | Accounts | v1 Tx      |
| ---------------- | -------- | ---------- |
|       2835 bytes |        9 | 3282 bytes |

## 3. Reclaim Dvp

| Function                  |   Total CU |     Net CU |
| ------------------------- | ---------- | ---------- |
| `cpi_spp_transact_signed` |    140,317 |    140,317 |
| `process_reclaim_dvp_ix`  |    244,755 |    104,438 |

**Proving Time**
| SPP proof | DvP circuit proof | Total  |
| --------- | ----------------- | ------ |
|    123 ms |             15 ms | 139 ms |

**Transaction Size**
| Instruction Data | Accounts | v1 Tx      |
| ---------------- | -------- | ---------- |
|        681 bytes |        9 | 1128 bytes |

## 4. Reject Dvp

| Function                  |   Total CU |     Net CU |
| ------------------------- | ---------- | ---------- |
| `cpi_spp_transact_signed` |    149,047 |    149,047 |
| `process_reject_dvp_ix`   |    254,645 |    105,598 |

**Proving Time**
| SPP proof | DvP circuit proof | Total  |
| --------- | ----------------- | ------ |
|    231 ms |             28 ms | 260 ms |

**Transaction Size**
| Instruction Data | Accounts | v1 Tx      |
| ---------------- | -------- | ---------- |
|        904 bytes |       11 | 1417 bytes |

## 5. Settle Dvp

| Function                  |   Total CU |     Net CU |
| ------------------------- | ---------- | ---------- |
| `cpi_spp_transact_signed` |    153,553 |    153,553 |
| `process_settle_dvp_ix`   |    258,614 |    105,061 |

**Proving Time**
| SPP proof | DvP circuit proof | Total  |
| --------- | ----------------- | ------ |
|    257 ms |             38 ms | 295 ms |

**Transaction Size**
| Instruction Data | Accounts | v1 Tx      |
| ---------------- | -------- | ---------- |
|       1226 bytes |       11 | 1739 bytes |

