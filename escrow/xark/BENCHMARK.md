# Timelock Escrow (Xark) -- CU Benchmark

1. Measure the timelock escrow escrow/withdraw instructions under Mollusk using the shielded-pool program built from Zolana af9d32923 and the Xark-backed SDK. Each instruction verifies its generated program proof, then CPIs SPP `transact` (the `cpi_spp_transact_signed` row). Only the timelock escrow program is profiled; the shielded-pool program is built plain, so the CU its CPI consumes is charged to the `cpi_spp_transact_signed` row as a black box.

2. Record SPP and escrow circuit proving times, consumed compute units, and serialized v1 transaction sizes. Escrow uses 1x2 (change and escrow UTXO); withdraw uses 1x2 with compact output padding.

3. Report compiled circuit constraints, checksummed Xark proving-key and verifying-key sizes. Proof times are warm, in the release profile. An escrow circuit proof time is the median of five Xark proofs after one warm-up proof, from one checked circuit copy. An SPP proof time is one sample after one warm-up proof on the running prover server. Build the profiling program first with cargo build-sbf --manifest-path program/Cargo.toml --tools-version v1.54 --sbf-out-dir target/bench -- --features bpf-entrypoint,profile-program.

Regenerate with `cargo test --release --locked -p timelock-escrow-test --test bench_cu -- --ignored --nocapture`.

## Definitions

- **Total CU**: Compute units consumed by the function including all children
- **Net CU**: Compute units consumed by the function itself (excluding children)

## Table of Contents

1. [Circuits](#circuits)
2. [Escrow](#escrow)
3. [Withdraw](#withdraw)

## 1. Circuits

| Circuit  | Constraints | Proving key | Verifying key | Warm proof time |
| -------- | ----------- | ----------- | ------------- | --------------- |
|   escrow |        8014 |     1.54 MB |         296 B |          188 ms |
| withdraw |        5422 |     1.13 MB |         296 B |          142 ms |

## 2. Escrow

| Function                  |   Total CU |     Net CU |
| ------------------------- | ---------- | ---------- |
| `cpi_spp_transact_signed` |    145,041 |    145,041 |
| `process_escrow_ix`       |    241,584 |     96,543 |

**Proving Time**
| Warm SPP proof | Warm escrow circuit proof | Total  |
| -------------- | ------------------------- | ------ |
|          63 ms |                    188 ms | 251 ms |

**Transaction Size**
| Instruction Data | Accounts | v1 Tx      |
| ---------------- | -------- | ---------- |
|        742 bytes |        8 | 1092 bytes |

## 3. Withdraw

| Function                  |   Total CU |     Net CU |
| ------------------------- | ---------- | ---------- |
| `cpi_spp_transact_signed` |    144,651 |    144,651 |
| `process_withdraw_ix`     |    242,482 |     97,831 |

**Proving Time**
| Warm SPP proof | Warm escrow circuit proof | Total  |
| -------------- | ------------------------- | ------ |
|          62 ms |                    142 ms | 204 ms |

**Transaction Size**
| Instruction Data | Accounts | v1 Tx     |
| ---------------- | -------- | --------- |
|        593 bytes |        9 | 944 bytes |

