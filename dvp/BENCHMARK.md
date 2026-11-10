# Private DvP -- CU Benchmark

1. Measure the five DvP instructions under Mollusk using the shielded-pool program built from Zolana 98948daa1 and the Xark-backed SDK. Each DvP handler verifies its generated program proof and invokes SPP directly. Cancellation and rejection share process_abort_dvp.

2. Record SPP and program proving times, consumed compute units, and serialized v1 transaction sizes. Create/reclaim use 1x2 with compact output padding; settle uses 3x4; cancel/reject use 3x2 with real optional deposits before random input padding.

3. Report compiled program constraints, checksummed Xark proving-key and verifying-key sizes. Proof times are warm, in the release profile. A DvP circuit proof time is the median of five Xark proofs after one warm-up proof, from one checked circuit copy; each Xark proof rereads the bytecode and proving key, rebuilds the rows, proves and checks the witness. An SPP proof time is one sample after one warm-up proof on the running prover server. Build the profiling program first with cargo build-sbf --manifest-path dvp/program/Cargo.toml --tools-version v1.54 --sbf-out-dir dvp/target/bench -- --features bpf-entrypoint,profile-program.

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
| `process_abort_dvp`      |    256,697 |    256,697 |
| `process_cancel_dvp_ix`  |    256,726 |         29 |

**Proving Time**
| Warm SPP proof | Warm DvP circuit proof | Total  |
| -------------- | ---------------------- | ------ |
|         143 ms |                 445 ms | 589 ms |

**Transaction Size**
| Instruction Data | Accounts | v1 Tx      |
| ---------------- | -------- | ---------- |
|        904 bytes |       11 | 1417 bytes |

## 2. Circuits

| Circuit     | Constraints | Proving key | Verifying key | Warm proof time |
| ----------- | ----------- | ----------- | ------------- | --------------- |
|  create_dvp |       13515 |     2.68 MB |         296 B |          278 ms |
| reclaim_dvp |       14546 |     2.84 MB |         296 B |          309 ms |
|  settle_dvp |       23004 |     4.67 MB |         296 B |          504 ms |
|   abort_dvp |       19392 |     4.12 MB |         296 B |          445 ms |

## 3. Create Dvp

| Function                 |   Total CU |     Net CU |
| ------------------------ | ---------- | ---------- |
| `process_create_dvp_ix`  |    243,346 |    243,346 |

**Proving Time**
| Warm SPP proof | Warm DvP circuit proof | Total  |
| -------------- | ---------------------- | ------ |
|          57 ms |                 278 ms | 335 ms |

**Transaction Size**
| Instruction Data | Accounts | v1 Tx      |
| ---------------- | -------- | ---------- |
|       2835 bytes |        9 | 3282 bytes |

## 4. Reclaim Dvp

| Function                 |   Total CU |     Net CU |
| ------------------------ | ---------- | ---------- |
| `process_reclaim_dvp_ix` |    244,786 |    244,786 |

**Proving Time**
| Warm SPP proof | Warm DvP circuit proof | Total  |
| -------------- | ---------------------- | ------ |
|          58 ms |                 309 ms | 367 ms |

**Transaction Size**
| Instruction Data | Accounts | v1 Tx      |
| ---------------- | -------- | ---------- |
|        681 bytes |        9 | 1128 bytes |

## 5. Reject Dvp

| Function                 |   Total CU |     Net CU |
| ------------------------ | ---------- | ---------- |
| `process_abort_dvp`      |    252,735 |    252,735 |
| `process_reject_dvp_ix`  |    252,764 |         29 |

**Proving Time**
| Warm SPP proof | Warm DvP circuit proof | Total  |
| -------------- | ---------------------- | ------ |
|         156 ms |                 433 ms | 590 ms |

**Transaction Size**
| Instruction Data | Accounts | v1 Tx      |
| ---------------- | -------- | ---------- |
|        904 bytes |       11 | 1417 bytes |

## 6. Settle Dvp

| Function                 |   Total CU |     Net CU |
| ------------------------ | ---------- | ---------- |
| `process_settle_dvp_ix`  |    260,822 |    260,822 |

**Proving Time**
| Warm SPP proof | Warm DvP circuit proof | Total  |
| -------------- | ---------------------- | ------ |
|         152 ms |                 504 ms | 656 ms |

**Transaction Size**
| Instruction Data | Accounts | v1 Tx      |
| ---------------- | -------- | ---------- |
|       1226 bytes |       11 | 1739 bytes |

