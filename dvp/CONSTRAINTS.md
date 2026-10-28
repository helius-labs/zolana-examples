# DvP constraint profile

Profiled with Xark `38c0de6`, Zolana `226ddcb7e`, and application commit
`fbd5899`. All four profiled bytecode files match the checked circuit artifacts.

`xark profile` reports compiler R1CS rows. The proof benchmark counts the row
source selected for proving, including Xark's reduction/minimization passes,
so the two totals differ.

| Circuit | Compiler rows | Proof benchmark rows | Poseidon rows | Range-check rows | Other rows |
| --- | ---: | ---: | ---: | ---: | ---: |
| create_dvp | 15,543 | 15,086 | 10,159 (65.4%) | 4,836 (31.1%) | 548 |
| reclaim_dvp | 17,012 | 16,350 | 11,151 (65.5%) | 5,651 (33.2%) | 210 |
| settle_dvp | 26,226 | 25,507 | 17,728 (67.6%) | 7,879 (30.0%) | 619 |
| abort_dvp | 22,559 | 21,822 | 15,474 (68.6%) | 6,665 (29.5%) | 420 |

Poseidon hashing dominates. Range checks include decoding bounded integer and
byte inputs, along with arithmetic and comparison checks. The remaining rows
cover equality, boolean operations, comparisons, and other multiplication.

## Create by call stage

These stages partition create's 15,543 compiler rows. The Poseidon costs above
occur inside these stages.

| Stage | Compiler rows |
| --- | ---: |
| Check terms and recompute the DvP ID (`assert_dvp`) | 6,147 |
| Decode and range-check typed inputs | 3,892 |
| Build transaction/output hashes and public hash (`check`) | 2,744 |
| Reserve the DvP address (`NewAddress::new`) | 1,341 |
| Check amounts, asset inequality, and time window (`assert_valid_trade`) | 1,179 |
| Check the instruction blinding seed | 238 |
| Register the state output | 1 |
| Bind the final public hash | 1 |

All four circuits spend 6,147 rows in `assert_dvp`, which recomputes the nested
terms commitment, including destinations, asset hashes, and identities.
Settlement additionally spends 6,949 rows in transaction checking, 3,450 in
loading its two token deposits, and 1,659 in loading and closing the DvP state.

The HTML reports drill down through each source line's call chain, such as
`execute → assert_dvp → hash → poseidon → pow`. Function costs are inclusive;
parent and child function totals overlap.

## Reproduce

Build the circuit artifacts first as described in `README.md`, then run from
`dvp/`:

```sh
for circuit in create_dvp reclaim_dvp settle_dvp abort_dvp; do
  .tools/xark/target/release/xark profile \
    "target/zk/dvp-program/.entries/$circuit" \
    --out "target/constraint-profiles/$circuit" \
    --html
done
```

Each output directory contains `profile.html` with the source heatmap and call
chains, and `profile.json` with per-row attribution. Add `--json` to print the
aggregated breakdown as JSON.
