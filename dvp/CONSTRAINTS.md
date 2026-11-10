# DvP constraint profile

Profiled with Xark 0.3.0 (CLI `13c3f0c`), Zolana `98948daa1`, and the circuits
in this commit. All four profiled bytecode files match the checked circuit
artifacts.

`xark profile` reports compiler R1CS rows. The proof benchmark counts the row
source selected for proving, including Xark's reduction/minimization passes,
so the two totals differ.

| Circuit | Compiler rows | Proof benchmark rows | Poseidon rows | Range-check rows | Other rows |
| --- | ---: | ---: | ---: | ---: | ---: |
| create_dvp | 13,527 | 13,515 | 8,716 (64.4%) | 4,290 (31.7%) | 521 |
| reclaim_dvp | 14,569 | 14,546 | 9,471 (65.0%) | 4,912 (33.7%) | 186 |
| settle_dvp | 23,059 | 23,004 | 15,563 (67.5%) | 6,911 (30.0%) | 585 |
| abort_dvp | 19,414 | 19,392 | 13,309 (68.6%) | 5,783 (29.8%) | 322 |

Poseidon hashing dominates. Range checks include decoding bounded integer and
byte inputs, along with arithmetic and comparison checks. The remaining rows
cover equality, boolean operations, comparisons, and other multiplication.

## Create by call stage

These stages partition create's 13,527 compiler rows. The Poseidon costs above
occur inside these stages.

| Stage | Compiler rows |
| --- | ---: |
| Check terms and recompute the DvP ID (`assert_dvp`) | 5,047 |
| Decode and range-check typed inputs | 3,490 |
| Build transaction/output hashes and public hash (`check`) | 2,004 |
| Check amounts, asset inequality, and time window (`assert_valid_trade`) | 1,167 |
| Reserve the DvP address (`NewAddress::new`) | 1,098 |
| Bind the private DvP address to the public owner hash (`dvp_owner`) | 482 |
| Check the instruction blinding seed | 238 |
| Bind the final public hash | 1 |

All four circuits spend 5,047 rows in `assert_dvp`, which recomputes the nested
terms commitment, including destinations, asset hashes, and identities, and
482 rows in `dvp_owner`. Settlement additionally spends 5,979 rows in transaction checking, 2,964 in
loading its two token deposits, and 1,410 in closing the DvP state.

The HTML reports drill down through each source line's call chain, such as
`execute → assert_dvp → hash → poseidon → pow`. Function costs are inclusive;
parent and child function totals overlap.

## Reproduce

Build the circuit artifacts first as described in `README.md`, then run from
`dvp/`:

```sh
for circuit in create_dvp reclaim_dvp settle_dvp abort_dvp; do
  .tools/zolana/target/release/xark profile \
    "target/zk/dvp-program/.entries/$circuit" \
    --out "target/constraint-profiles/$circuit" \
    --html
done
```

Each output directory contains `profile.html` with the source heatmap and call
chains, and `profile.json` with per-row attribution. Add `--json` to print the
aggregated breakdown as JSON.
