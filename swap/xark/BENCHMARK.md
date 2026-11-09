# Confidential Swap -- CU Benchmark

Compute unit profiling for the confidential swap make/take/cancel instructions, replayed under mollusk, with the Xark circuits and the shielded-pool program built from Zolana af9d32923. The shielded-pool tree account is built directly (the program's `create_tree` init plus the input utxo hashes appended), and each instruction hashes its public input, verifies its own Groth16 proof, then CPIs SPP `transact` (the `cpi_spp_transact*` row). Only the swap program is profiled; the shielded-pool program is built plain, so the CU its CPI consumes is charged to the `cpi_spp_transact*` row as a black box and its internal functions do not appear here. Each instruction section also records its proving times (SPP transfer proof plus swap circuit proof, each the second of two proofs) and the serialized size of the version 1 transaction these instructions are sent as, which states its compute ceilings in the message header and may run to 4096 bytes.

Regenerate with `cargo test --release --locked -p swap-test --test bench_cu -- --ignored --nocapture`.

## Definitions

- **Total CU**: Compute units consumed by the function including all children
- **Net CU**: Compute units consumed by the function itself (excluding children)

## Table of Contents

1. [Cancel](#cancel)
2. [Circuits](#circuits)
3. [Make](#make)
4. [Take](#take)

## 1. Cancel

| Function                  |   Total CU |     Net CU |
| ------------------------- | ---------- | ---------- |
| `cpi_spp_transact_signed` |    142,419 |    142,419 |
| `process_cancel_ix`       |    240,271 |     97,852 |

**Proving Time**
| SPP transfer proof | Swap circuit proof | Total  |
| ------------------ | ------------------ | ------ |
|              59 ms |             372 ms | 432 ms |

**Transaction Size**
| Instruction Data | Accounts | v1 Tx     |
| ---------------- | -------- | --------- |
|        624 bytes |        9 | 975 bytes |

## 2. Circuits

| Circuit | Constraints | Proving key | Verifying key | Warm proof time |
| ------- | ----------- | ----------- | ------------- | --------------- |
|    make |        3928 |    760990 B |         296 B |          401 ms |
|    take |        7102 |     1.40 MB |         296 B |          726 ms |
|  cancel |        3791 |    739550 B |         296 B |          372 ms |

## 3. Make

| Function                  |   Total CU |     Net CU |
| ------------------------- | ---------- | ---------- |
| `cpi_spp_transact_signed` |    147,818 |    147,818 |
| `process_make_ix`         |    244,839 |     97,021 |

**Proving Time**
| SPP transfer proof | Swap circuit proof | Total  |
| ------------------ | ------------------ | ------ |
|             103 ms |             401 ms | 504 ms |

**Transaction Size**
| Instruction Data | Accounts | v1 Tx      |
| ---------------- | -------- | ---------- |
|        899 bytes |        9 | 1282 bytes |

## 4. Take

| Function                  |   Total CU |     Net CU |
| ------------------------- | ---------- | ---------- |
| `cpi_spp_transact_signed` |    148,721 |    148,721 |
| `process_take_ix`         |    246,095 |     97,374 |

**Proving Time**
| SPP transfer proof | Swap circuit proof | Total  |
| ------------------ | ------------------ | ------ |
|             101 ms |             726 ms | 827 ms |

**Transaction Size**
| Instruction Data | Accounts | v1 Tx      |
| ---------------- | -------- | ---------- |
|        806 bytes |       10 | 1190 bytes |

