# Private DvP -- CU Benchmark

Compute unit profiling for the private DvP create_dvp/reclaim_dvp/settle_dvp/cancel_dvp/reject_dvp instructions, replayed under mollusk. The shielded-pool tree account is built directly (the program's `create_tree` init plus the input utxo hashes appended), and each instruction verifies its own Groth16 proof, then CPIs SPP `transact` (the `cpi_spp_transact*` row). Only the DvP program is profiled; the shielded-pool program is the plain v0.4.0-alpha release build, so the CU its CPI consumes is charged to the `cpi_spp_transact*` row as a black box and its internal functions do not appear here. The SPP shapes are 1x2 for create_dvp (the address slot in, the DvP UTXO and compact padding out) and reclaim_dvp (a deposit in, the refund and compact padding out), 3x4 for settle_dvp, and 3x2 for cancel_dvp (both deposits) and reject_dvp (the payment deposit only, so one padding input). Each instruction section also records its proving times (SPP proof plus the DvP circuit proof) and the serialized size of the version 1 transaction it is sent as, which states its compute ceilings in the message header and may run to 4096 bytes. The Circuits section lists each DvP circuit's R1CS constraint count, its proving and verifying key sizes and its circuit proof time (abort_dvp, which serves cancel_dvp and reject_dvp, as timed for cancel_dvp). The profiled program must be built first: `cargo build-sbf --manifest-path dvp/program/Cargo.toml --sbf-out-dir dvp/target/bench -- --features bpf-entrypoint,profile-program`.

Regenerate with `cargo test -p dvp-test --test bench_cu -- --ignored --nocapture`.

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

| Function                  |   Total CU |     Net CU |
| ------------------------- | ---------- | ---------- |
| `cpi_spp_transact_signed` |    150,791 |    150,791 |
| `process_cancel_dvp_ix`   |    254,889 |    104,098 |

**Proving Time**
| SPP proof | DvP circuit proof | Total  |
| --------- | ----------------- | ------ |
|    250 ms |             33 ms | 283 ms |

**Transaction Size**
| Instruction Data | Accounts | v1 Tx      |
| ---------------- | -------- | ---------- |
|        904 bytes |       11 | 1417 bytes |

## 2. Circuits

| Circuit     | Constraints | Proving key | Verifying key | Proof time |
| ----------- | ----------- | ----------- | ------------- | ---------- |
|  create_dvp |        7415 |     1.33 MB |         716 B |      96 ms |
| reclaim_dvp |        5120 |     1.04 MB |         716 B |      18 ms |
|  settle_dvp |       18207 |     3.58 MB |         716 B |      41 ms |
|   abort_dvp |       10247 |     2.08 MB |         716 B |      33 ms |

## 3. Create Dvp

| Function                  |   Total CU |     Net CU |
| ------------------------- | ---------- | ---------- |
| `cpi_spp_transact_signed` |    140,806 |    140,806 |
| `process_create_dvp_ix`   |    243,021 |    102,215 |

**Proving Time**
| SPP proof | DvP circuit proof | Total  |
| --------- | ----------------- | ------ |
|     90 ms |             96 ms | 186 ms |

**Transaction Size**
| Instruction Data | Accounts | v1 Tx      |
| ---------------- | -------- | ---------- |
|       2835 bytes |        9 | 3282 bytes |

## 4. Reclaim Dvp

| Function                  |   Total CU |     Net CU |
| ------------------------- | ---------- | ---------- |
| `cpi_spp_transact_signed` |    141,764 |    141,764 |
| `process_reclaim_dvp_ix`  |    244,702 |    102,938 |

**Proving Time**
| SPP proof | DvP circuit proof | Total  |
| --------- | ----------------- | ------ |
|    154 ms |             18 ms | 172 ms |

**Transaction Size**
| Instruction Data | Accounts | v1 Tx      |
| ---------------- | -------- | ---------- |
|        681 bytes |        9 | 1128 bytes |

## 5. Reject Dvp

| Function                  |   Total CU |     Net CU |
| ------------------------- | ---------- | ---------- |
| `cpi_spp_transact_signed` |    150,124 |    150,124 |
| `process_reject_dvp_ix`   |    255,722 |    105,598 |

**Proving Time**
| SPP proof | DvP circuit proof | Total  |
| --------- | ----------------- | ------ |
|    253 ms |             31 ms | 285 ms |

**Transaction Size**
| Instruction Data | Accounts | v1 Tx      |
| ---------------- | -------- | ---------- |
|        904 bytes |       11 | 1417 bytes |

## 6. Settle Dvp

| Function                  |   Total CU |     Net CU |
| ------------------------- | ---------- | ---------- |
| `cpi_spp_transact_signed` |    152,454 |    152,454 |
| `process_settle_dvp_ix`   |    259,015 |    106,561 |

**Proving Time**
| SPP proof | DvP circuit proof | Total  |
| --------- | ----------------- | ------ |
|    292 ms |             41 ms | 334 ms |

**Transaction Size**
| Instruction Data | Accounts | v1 Tx      |
| ---------------- | -------- | ---------- |
|       1226 bytes |       11 | 1739 bytes |

