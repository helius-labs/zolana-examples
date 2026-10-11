# Stake Pool Intents

Draft.

- Confidential deposits into and liquid withdrawals from one SPL Stake Pool. Users lock SOL or pool token UTXOs, encrypt the escrow UTXOs with the crank's handoff key, and go offline.
- Each batch contains intents from one period and one direction. The program collects their escrowed assets, calls Stake Pool `DepositSol` or `WithdrawSol` by CPI, and returns confidential payout UTXOs in one atomic transaction.
- The crank supplies fees and rent; the escrowed balances fund the pool operation.
- Pool and application fees are assumed constant and accepted by users. Users receive the pool's deterministic conversion after fees and rounding, with no per-user `min_out`.
- The prepared proofs include the expected pool output. A different execution result fails the transaction and requires new proofs.
- Start with two intents. Reuse the staged input and output records from draft [Zolana PR 407](https://github.com/helius-labs/zolana/pull/407), branch `jorrit/spp-output-buffer-scaling`. Settlement uses two Solana Privacy Program (SPP) proofs and one application proof through existing SPP `transact` instructions.
- The `create_intent` proof checks that the recipient `owner_hash` equals the source UTXO's owner. The escrow authority's `nullifier_secret` is 0, as in [kLend intents](https://github.com/helius-labs/zolana/blob/ad40d2777cd87256e0d2aa2f112e4bc00072c3fd/sdk-tests/k-lend/intent/spec.md).
- Unsettled intents refund after expiry. Successful payouts leave permanent receipts from which users reconstruct their payout UTXOs.

## Actors

| Actor | Role | Trust |
| --- | --- | --- |
| User | Locks an input UTXO, retains the decrypted escrow UTXO, recovers the payout or cancels after expiry. | None. The proofs check allocation and destination. |
| Crank | Decrypts escrow UTXOs, prepares proofs, stages batches, and submits settlement. Learns individual amounts. | Liveness and privacy. |
| Anyone | Submits a sealed batch or proves an expired refund with the decrypted escrow UTXO. | None. The proofs check destinations. |

## Periods

Period `p` covers slots `[p * period_slots, (p + 1) * period_slots)`. Its expiry is `(p + 1) * period_slots + settle_slots`.

1. **Collect**: users create intents during `p`. The period account stores their `escrow_utxo_hash` values and resolution status.
2. **Stage**: the crank groups compatible intents, prepares proofs, and submits `write_batch` and `seal_batch` instructions.
3. **Settle**: after collection ends and before expiry, one transaction collects, executes, and pays. Only its included intents resolve.
4. **Cancel**: at `Clock.slot >= expiry`, unresolved intents refund. Staging cannot reserve intents or extend expiry.

## Instructions

| Instruction | Description | Accounts Read | Accounts Modified | Access control |
| --- | --- | --- | --- | --- |
| create_market | Stores pool, program, mint, classic SPL Token program, handoff key, fees, treasury, periods, and capacities. Creates a zero-value control UTXO. Requires unrestricted pool funding authorities. | Stake Pool state | market, control | Authority signs |
| create_period | Creates bounded intent storage for period `p`. | market | period | Crank funds rent |
| create_intent | A `create_intent` proof and SPP `1x2` create escrow and change during `p`. The escrow's `data_hash` includes market, period, direction, assets, amount, and recipient. Includes the escrow UTXO encrypted with the handoff key. | market | period, SPP tree | Source owner signs |
| init_batch | Creates staging storage, a PDA SOL vault, its associated pool token account, and a separate funded SPP payer PDA. | market, period | batch, vaults, payer | Staging authority signs |
| write_batch | Writes bounded collect, pay, proof, and recovery records. | batch | batch | Staging authority signs |
| seal_batch | Requires complete records, distinct active intent indices, supported shapes, and canonical padding. Freezes the instruction data and proofs. | market, period | batch | Staging authority signs |
| settle_batch | Verifies the application proof, collects through SPP, executes Stake Pool, transfers fees, and pays through SPP. Checks exact deltas and restores vault balances. Failure rolls back CPI transfers and intent resolution. | market, sealed batch | period, control, batch, receipt, vaults, pool, SPP trees | Permissionless; program authorizes PDAs with `invoke_signed` |
| cancel_intent | From expiry, a refund proof and SPP `1x2` return the input amount to the recipient in the escrow terms, with compact padding. | market | period, SPP tree | Permissionless with decrypted escrow UTXO |
| abort_batch / close_batch | Releases unsuccessful or settled staging storage and unused fee funds to their recorded funder. Successful receipts remain. | batch | batch, vaults, payer | Staging authority signs |
| close_period | Releases period storage after collection ends and its intents resolve. | market | period | Rent returns to recorded funder |

## Batch settlement

Reuse PR 407's SPP `transact` assembler inside the staking program, which enforces intents before signing its PDAs. Collect publishes escrow inputs and no live outputs. Pay consumes an existing, real zero-value control UTXO and recreates it at slot 0; payouts occupy slots `1..N`. Remaining slots are compact padding.

Two intents use collect `2x2` and pay `1x4`; four use `4x2` and `1x8`. Both proofs use a fixed, crank-funded SPP `payer` PDA, so the Solana transaction payer can change. Publish the decrypted replacement control UTXO; keep pay's `blinding_seed` secret.

The application proof checks active `escrow_utxo_hash` values and derived `nullifier_hash` values, then recomputes both SPP `private_tx_hash` and `external_data_hash` values from the same inputs and outputs. Its `public_input_hash` includes market, period, batch, assets, signers, vaults, payout owners, control UTXO hashes, recovery records, and allocation amounts. The program checks measured pool output against the prepared amount. SPP checks membership, authorization, nullifiers, and conservation.

For inputs `a_i`, aggregate input `A`, measured output `M`, and fixed application `fee_bps`:

```text
A = sum(a_i)
F = floor(M * fee_bps / 10000)
payout_amount_i = floor(a_i * (M - F) / A)
R = M - F - sum(payout_amount_i)
```

Pool fees are already reflected in `M`. Transfer `F + R` to the treasury and shield `sum(payout_amount_i)` for users. The proof checks this allocation, nonzero inputs and payouts, and `0 <= R < N`, using checked integer arithmetic. Balance deltas exclude existing vault donations.

## Recovery

Staged outputs have `data: None`. The proposed receipt stores the output tree, aggregate amounts, fees, and each intent's `escrow_utxo_hash`, payout slot, `payout_utxo_hash`, and `masked_payout_blinding_i`:

```text
masked_payout_blinding_i = payout_blinding_i
  + Poseidon(recovery_domain, escrow_blinding_i, recovery_context_hash, payout_slot_i) mod BN254_scalar_modulus
```

`payout_blinding_i` is the final SPP output blinding. The receipt's `recovery_context_hash` hashes the market, period, batch ID, aggregate amounts, output tree, and both SPP statements.

The proof checks each mask against the payout blinding. With their retained decrypted escrow UTXO and receipt, users reconstruct the payout UTXO, compare its `utxo_hash`, and obtain a Merkle proof. The SDK ingests receipts.

Cancel derives its `blinding_seed` from a cancellation domain and `escrow_blinding`. Use the standard SPP output derivations with the actual `first_nullifier` and output tree. The user reconstructs the refund UTXO from the retained decrypted escrow UTXO.

## Visibility

| Field | Visibility | Reason |
| --- | --- | --- |
| Market, period, pool, direction | Public | Batch and CPI accounts |
| Intent UTXO hashes and recipient owner tags | Public | Period records and inline SPP outputs |
| Individual input and payout amounts | Private | UTXO hashes and private proof inputs |
| Aggregate amounts, fees, execution time | Public | Vault transfers and receipt |
| Settle or cancel | Public | Intent resolution status |
| Decrypted escrow UTXO | User and crank | Handoff encrypted with the crank's key |

## Risks

1. Reserve liquidity: `WithdrawSol` requires available SOL. Insufficient liquidity reverts the batch; users can later cancel for their original pool tokens.
2. Small batches: knowing the other users' inputs reveals the remaining amount. Default-ring owners are public.
3. Stale preparation: changes to pool accounting, tree roots, or the control UTXO require new proofs. Intent status and nullifier checks reject competing settlements or cancellations.
4. Recovery: successful receipts must remain available after closing staging storage.

## Future Work

1. Review the proposed recovery masks. Integration tests on `jorrit/spp-output-buffer-scaling` with the real [Stake Pool program](https://github.com/solana-program/stake-pool/blob/8abfd17428728bc4c1ce181c238a3357bab82ad5/program/src/instruction.rs): two-user deposit and withdrawal, payout spends, receipt recovery, expiry refunds, and rollback.
2. Four-user and wider batches. Explicitly select PR 407's wide shapes and measure the complete transaction's bytes, accounts, CPI data, heap, trace entries, and CU.
3. Extend the fixture's assembler for empty collect outputs, complete-write tracking, and sealed instruction data.
4. Mixed-direction netting, multiple pools, and hiding pool or direction.
5. `WithdrawStake` with stake accounts and delayed deactivation.
