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
| `cpi_spp_transact_signed` |    151,160 |    151,160 |
| `process_cancel_dvp_ix`   |    255,266 |    104,106 |

**Proving Time**
| SPP proof | DvP circuit proof | Total  |
| --------- | ----------------- | ------ |
|    236 ms |             27 ms | 263 ms |

**Transaction Size**
| Instruction Data | Accounts | v1 Tx      |
| ---------------- | -------- | ---------- |
|        904 bytes |       11 | 1417 bytes |

## 2. Create Dvp

| Function                  |   Total CU |     Net CU |
| ------------------------- | ---------- | ---------- |
| `cpi_spp_transact_signed` |    140,444 |    140,444 |
| `process_create_dvp_ix`   |    242,642 |    102,198 |

**Proving Time**
| SPP proof | DvP circuit proof | Total  |
| --------- | ----------------- | ------ |
|     65 ms |             81 ms | 146 ms |

**Transaction Size**
| Instruction Data | Accounts | v1 Tx      |
| ---------------- | -------- | ---------- |
|       2835 bytes |        9 | 3282 bytes |

## 3. Reclaim Dvp

| Function                  |   Total CU |     Net CU |
| ------------------------- | ---------- | ---------- |
| `cpi_spp_transact_signed` |    140,325 |    140,325 |
| `process_reclaim_dvp_ix`  |    243,265 |    102,940 |

**Proving Time**
| SPP proof | DvP circuit proof | Total  |
| --------- | ----------------- | ------ |
|    126 ms |             16 ms | 143 ms |

**Transaction Size**
| Instruction Data | Accounts | v1 Tx      |
| ---------------- | -------- | ---------- |
|        681 bytes |        9 | 1128 bytes |

## 4. Reject Dvp

| Function                  |   Total CU |     Net CU |
| ------------------------- | ---------- | ---------- |
| `cpi_spp_transact_signed` |    149,786 |    149,786 |
| `process_reject_dvp_ix`   |    253,892 |    104,106 |

**Proving Time**
| SPP proof | DvP circuit proof | Total  |
| --------- | ----------------- | ------ |
|    233 ms |             29 ms | 263 ms |

**Transaction Size**
| Instruction Data | Accounts | v1 Tx      |
| ---------------- | -------- | ---------- |
|        904 bytes |       11 | 1417 bytes |

## 5. Settle Dvp

| Function                  |   Total CU |     Net CU |
| ------------------------- | ---------- | ---------- |
| `cpi_spp_transact_signed` |    152,464 |    152,464 |
| `process_settle_dvp_ix`   |    257,549 |    105,085 |

**Proving Time**
| SPP proof | DvP circuit proof | Total  |
| --------- | ----------------- | ------ |
|    247 ms |             37 ms | 285 ms |

**Transaction Size**
| Instruction Data | Accounts | v1 Tx      |
| ---------------- | -------- | ---------- |
|       1226 bytes |       11 | 1739 bytes |

