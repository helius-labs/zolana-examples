# Timelock Escrow Program

A timelock escrow on the Solana Privacy Program (SPP): a creator locks a
private balance as a shielded UTXO with an unlock timestamp, and withdraws it
once that timestamp has passed.

The escrow program is an SPP ZK program: it verifies a small proof of its own
escrow rules and delegates the confidential transfer to SPP. It stores no state
and owns no accounts.

See [`timelock_escrow.md`](timelock_escrow.md) for the full design: the privacy
model, escrow terms, instructions, and circuits.

## Implementations

- [`gnark/`](gnark/) — circuits written in Go with gnark, proved in process
  through `spr-gnark-ffi-prover`. CU numbers are in
  [`gnark/BENCHMARK.md`](gnark/BENCHMARK.md).
