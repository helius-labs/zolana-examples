# Stake Pool Intents

Private SOL staking lets users stake SOL without revealing their individual staked amounts.

Flow:

1. Users submit encrypted deposit or withdrawal requests, lock their SOL or liquid staking tokens in confidential UTXOs, and pay a fixed SOL fee.
2. Once per epoch, a crank settles each hash chain, matching deposits against withdrawals and executing the chain's net stake pool deposit or withdrawal.
3. Users receive liquid staking tokens for deposits or SOL for withdrawals, as confidential UTXOs.

## Settlement flow

1. **Lock.** After submissions close, the configured crank locks the epoch for a fixed settlement window, initially 10 minutes. Cancellation requires an unlocked epoch. The deadline cannot be extended.
2. **Execute each hash chain.** Prove coverage of every escrow UTXO in order, withdraw active escrow funds from SPP into the public vaults, execute the net pool operation, and deposit the confidential user payouts into SPP. Pay the recorded crank, mark the chain as executed, increment the epoch's resolved intent count by the number of newly spent escrows, and close its batch account, returning rent to the batch's recorded payer. These steps are atomic.
3. **Close the epoch PDA.** Check that `resolved_intent_count == intent_count`, pay the remaining SOL intent fees to the configured crank, then close the account and return epoch rent to `rent_payer`.

### Properties

1. The settlement window ends 10 minutes after submissions close, whether or not a crank locks or executes the epoch. Anyone can unlock an expired epoch.
2. Users can cancel an unspent intent whenever `epoch.locked == false`, without a cancellation deadline. Its SOL intent fee is not refunded.
3. A failed chain transaction leaves its escrow UTXOs unspent.
4. Each hash chain settles independently and can perform one net pool operation.
5. Proof generation and transaction submission can run in parallel. Solana serializes transactions that write to the same stake pool or epoch accounts.
6. Already-nullified escrows remain in the ordered proof but are excluded from SPP inputs, settlement totals, and payouts. The proof derives each nullifier from its escrow UTXO and checks inclusion under an authenticated SPP nullifier-tree root, or the program verifies its canonical, SPP-owned nullifier PDA and account layout for a queued spend. Fees retained for cancelled intents are paid to the configured crank when the epoch PDA closes.
7. Each Solana epoch can have multiple epoch PDAs, each with `M` hash chains of up to `N` intents. Pages share the epoch's deadlines and settle independently.
8. Cancellations before locking can invalidate a prepared settlement proof. The crank regenerates it with the cancelled escrow skipped.
9. The escrow withdrawal and payout deposit are separate SPP calls around the pool CPI. Each SPP call verifies its own proof; the program also verifies the settlement logic proof.

## Constants

```rust
const N: usize = 16;
const M: usize = 282;
```

The Epoch PDA uses a packed 82-byte header and 36 bytes per hash chain. `M = (10_240 - 82) / 36`, giving 10,234 bytes per page. The page cap follows Solana's [10 KiB account growth limit per instruction](https://solana.com/docs/core/accounts#limits).

## Accounts

1. **Rent payer PDA**

   System-owned SOL account with empty account data. Funds epoch rent and receives it back when the epoch closes. Anyone can fund it with a SOL transfer.

   Seeds: `[b"rent_payer", config]`.

2. **Escrow authority PDA**

   PDA signer with empty account data. Authorizes SPP spends of individual escrow UTXOs after the program verifies the settlement or cancellation proof.

   Seeds: `[b"escrow_authority", config]`.

3. **SOL vault PDA**

   System-owned SOL account with empty account data. Holds aggregate SOL during execution and authorizes transfers from the liquid staking token vault. Reused across batches; settlement returns its balance to its value before execution.

   Seeds: `[b"sol_vault", config]`.

4. **Liquid staking token vault**

   Associated token account for the SOL vault PDA, configured token program, and pool mint. Uses the standard SPL Token account layout. Receives aggregate escrow tokens from SPP and newly minted tokens from `DepositSol`, supplies tokens for `WithdrawSol`, and funds confidential payouts. Settlement returns its balance to its value before execution. Reused across batches.

5. **Config PDA**

   Created by `create_config` and retained across epochs.

   Seeds: `[b"config", stake_pool]`.

```rust
struct Config {
    discriminator: u8,
    authority: Address,
    stake_pool_program_id: Address,
    stake_pool: Address,
    pool_mint: Address,
    token_program_id: Address,
    crank: ShieldedAddress,
    intent_fee_lamports: u64,
    settlement_window_seconds: u32,
}
```

   `ShieldedAddress` contains `signing_pubkey`, `nullifier_pubkey`, and `viewing_pubkey`. The crank's signing public key must support Solana transaction or CPI signing. `lock_epoch` requires that signer; encryption uses `crank.viewing_pubkey`.

6. **Epoch PDA**

   One page of intents for a Solana epoch, created by `create_epoch` using rent from `rent_payer`. Stores the fixed deadlines and progress for its hash chains. Closed independently by `close_epoch` after submissions close and its intents resolve.

   Seeds: `[b"epoch", config, epoch_id, page_index]`.

```rust
struct Epoch {
    discriminator: u8,
    config: Address,
    epoch_id: u64,
    page_index: u64,
    submission_deadline: i64,
    settlement_deadline: i64,
    locked: bool,
    intent_count: u32,
    resolved_intent_count: u32,
    fee_balance_lamports: u64,
    hash_chains: [HashChain; M],
}

struct HashChain {
    utxo_hash_chain: [u8; 32],
    intent_count: u16,
    executed: bool,
    batch_exists: bool,
}
```

   Deadlines are Unix timestamps. `hash_chains` has exactly `M` entries. `resolved_intent_count` starts at zero. `cancel_intent` increments it by one after spending an escrow; `execute_batch` increments it by the proof-checked number of newly spent escrows and sets the chain's `executed` flag. Already-nullified escrows are not counted again. `close_epoch` requires `resolved_intent_count == intent_count` and pays the remaining `fee_balance_lamports` to the configured crank before returning rent.

7. **Batch PDA**

   Created by `create_batch` for one hash chain. `fill_batch` appends SPP input records and their payout outputs. Successful `execute_batch` closes it and returns rent to its recorded payer; `close_epoch` cleans up unexecuted batches after refunds.

   Seeds: `[b"batch", epoch, hash_chain_index]`.

```rust
struct Batch {
    discriminator: u8,
    epoch: Address,
    hash_chain_index: u16,
    utxo_hash_chain: [u8; 32],
    intent_count: u16,
    crank: ShieldedAddress,
    rent_payer: Address,
    tx_viewing_pk: [u8; 33],
    salt: [u8; 16],
    utxos: Vec<(InputUtxo, Option<TransactOutput>)>,
}
```

   `InputUtxo` is the SPP interface type containing `nullifier_hash` and `tree_index`. `Some(output)` means settle the escrow; `None` means it was cancelled. The account is allocated for `N` entries with optional payout outputs, including the fixed ciphertext capacity for both user and crank. `tx_viewing_pk` and `salt` are the encryption parameters for the stored ciphertexts. `execute_batch` constructs the SPP instructions from these fields, the supplied proofs, root indexes, and aggregate amounts. Input tree accounts and nullifier PDAs are execution accounts. Execution checks the batch hash and count against the epoch entry.

User escrow and payout UTXOs are stored in SPP trees. Their encrypted data is published through SPP for both the user and the crank, so they can retrieve and decrypt the UTXOs after the epoch and batch accounts close. The proofs check the encryption for both recipients. Settlement proofs check that each payout is owned by the escrow's recorded recipient and that its user ciphertext encrypts the complete payout UTXO using `recipient.viewing_pubkey`. Settlement and refund outputs have `data_hash = 0`; only `create_intent` creates epoch-linked escrow UTXOs.

## Instructions

| Instruction | Description | Access |
| --- | --- | --- |
| `create_config` | Registers the stake pool, liquid staking token mint, crank shielded address, fixed `intent_fee_lamports`, `settlement_window_seconds` (initially 600), and a `rent_payer` PDA. | Authority |
| `create_epoch` | During the current Solana epoch's submission window, creates page `0`, or the next page after verifying that the preceding page for the same config and epoch is full. Initializes `M` hash-chain slots, intent and resolved intent counts, execution status, the epoch's shared deadlines, and SOL fee balance. The `rent_payer` PDA funds its rent. | Anyone; program signs for `rent_payer` |
| `create_intent` | Receives an epoch PDA and selects an available hash chain in the program. The intent proof requires the escrow at a fixed SPP output slot; its asset determines deposit or withdrawal, and its `data_hash` includes the epoch PDA and recipient. Checks encryption to the user and crank, locks SOL or liquid staking tokens, appends `escrow_utxo_hash`, and pays the fixed SOL fee atomically. | User |
| `lock_epoch` | After submissions close and before the shared settlement deadline, sets `locked = true` on the selected epoch PDA, preventing cancellations during settlement. | Configured crank signs |
| `unlock_epoch` | After the settlement deadline, sets `locked = false` so users can cancel remaining unspent intents. | Anyone |
| `create_batch` | During the locked settlement window, allocates space for `N` entries for one nonempty, unexecuted hash chain. Records its epoch, index, hash, intent count, crank, rent payer, and payout encryption parameters, with an empty `utxos` vector. | Configured crank signs |
| `fill_batch` | During the locked settlement window, appends `(input, Some(output))` for an escrow to settle or `(input, None)` for a cancelled escrow, within the batch's allocated capacity. | Recorded crank signs |
| `execute_batch` | During the locked settlement window, verifies the program-logic proof, builds an SPP withdrawal CPI from active entries, executes the net pool operation, and builds an SPP payout deposit CPI from stored outputs. Marks the chain as executed, increments `resolved_intent_count` only for entries with outputs, pays the recorded crank, closes the batch, and returns rent to the recorded payer, atomically. | Anyone with valid proofs |
| `cancel_intent` | Requires `epoch.locked == false`. Checks the escrow's epoch link, refunds it to destinations authorized by its recorded recipient, and increments `resolved_intent_count` by one atomically. The SOL intent fee stays in the epoch PDA. | Recipient-authorized refund |
| `close_epoch` | After submissions close, checks `resolved_intent_count == intent_count`, pays the selected epoch PDA's remaining SOL intent fees to the configured crank, then closes that page and returns its rent to `rent_payer`. Cleans up its unexecuted batch accounts, returning their rent to their recorded payers. | Anyone |

Each hash-chain slot holds at most `N` intents. Execution must reproduce the occupied chains with their exact counts and order.

Epoch creation is restricted to the current submission window, so a closed past epoch cannot be recreated.

Each hash chain executes once. When its net amount is zero, execution settles the confidential payouts and pays the crank while skipping the pool operation.

### create_intent

Transfers SOL or liquid staking tokens from a user's private balance into one escrow UTXO, appends its hash to an epoch hash chain, and pays the fixed SOL fee.

**Accounts**

1. `payer` — pays the SOL intent fee; signer, writable.
2. `config` — read; supplies the pool mint, crank address, and fee.
3. `epoch` — writable; receives the fee and records the escrow hash.
4. `escrow_authority` — the escrow UTXO's owner; signs the SPP CPI through the program.
5. `system_program` — read; transfers the SOL fee.
6. `spp_accounts` — the SPP `transact` account list, including its program, output tree, input trees, nullifier PDAs, and required owner signers.

**Instruction data**

```rust
struct CreateIntentIxData {
    intent_proof: IntentProof,
    spp: SppTransactIxData,
}
```

**UTXO data**

```rust
struct IntentUtxoData {
    epoch: Address,
    recipient: ShieldedAddress,
}
```

`data_hash` is the Poseidon hash of the canonical field encodings of `epoch` and `recipient`, including the recipient's signing, nullifier, and viewing public keys, with the `stake_pool_intent` domain tag. Amount, asset, and blinding remain in the standard UTXO fields. The asset determines deposit or withdrawal. The program selects the hash chain after proof generation.

**Proof**

The intent proof checks:

1. The fixed escrow output slot contains a nonzero UTXO owned by `escrow_authority`, with `nullifier_secret = 0`.
2. Its asset is SOL or the configured pool mint.
3. Its `data_hash` matches `IntentUtxoData`, and `epoch` matches the supplied epoch PDA.
4. The user and crank ciphertexts encrypt the same complete escrow UTXO and intent data, using `recipient.viewing_pubkey` and `config.crank.viewing_pubkey`.
5. The escrow output hash and ciphertexts match the submitted SPP transaction. The proof checks the same `private_tx_hash` passed to SPP.
6. Each real funding input has a signing key distinct from `escrow_authority`.

The program computes the proof's public inputs from the config, epoch PDA, and SPP instruction data. SPP verifies input authorization and asset conservation.

Creation requires an unlocked epoch PDA before its submission deadline and an available hash-chain slot. The program selects the slot, invokes SPP, transfers `intent_fee_lamports` from `payer` to `epoch`, appends the fixed output slot's `utxo_hash`, and increments the chain count, epoch count, and fee balance atomically.

### cancel_intent

Requires `epoch.locked == false` and refunds one unspent escrow to destinations authorized by its recorded recipient. Increments the epoch's resolved intent count. The SOL intent fee stays in the epoch PDA for the crank.

**Accounts**

1. `payer` — pays transaction fees; signer, writable.
2. `config` — read; supplies the escrow authority and crank address.
3. `epoch` — writable; checks the escrow's epoch link and lock flag, and increments `resolved_intent_count`.
4. `escrow_authority` — signs the SPP CPI through the program.
5. `spp_accounts` — the SPP `transact` account list.

**Instruction data**

```rust
struct CancelIntentIxData {
    cancel_proof: CancelIntentProof,
    spp: SppTransactIxData,
}
```

**Proof**

The cancellation proof checks:

1. The escrow is owned by `escrow_authority`, has SOL or the configured pool mint as its asset, and its `data_hash` matches `IntentUtxoData`. Its `epoch` matches the supplied epoch PDA.
2. The recipient authorizes cancellation, covering the epoch PDA, escrow hash, and SPP transaction, including its refund destinations.
3. The escrow is the only real SPP input. Its complete amount and asset are returned as confidential outputs to the authorized refund recipients, with `data_hash = 0`.
4. The refund ciphertexts encrypt the complete output UTXOs to the authorized recipients' viewing keys and the configured crank. Output hashes and ciphertexts match the submitted SPP transaction, using the same `private_tx_hash`.

The program checks `epoch.locked == false`, invokes SPP to spend the escrow and create the authorized refund outputs, and increments `resolved_intent_count` by one atomically. Cancellation has no deadline check and leaves `fee_balance_lamports` unchanged. SPP rejects an already-spent escrow. The user needs the epoch PDA from the escrow data, without a hash-chain index or membership proof.

### create_batch

Creates a batch PDA for one hash chain, allocating space for `N` entries and recording the payout encryption parameters. `fill_batch` uploads the entries; `execute_batch` reads them, verifies the supplied proofs, and settles the chain.

**Accounts**

1. `payer` — funds batch rent; signer, writable. Receives the rent when the batch closes.
2. `crank` — must match `config.crank`'s Solana signing address; signer.
3. `config` — read; supplies the configured crank.
4. `epoch` — writable; supplies the hash chain and settlement deadline, and records `batch_exists`.
5. `batch` — created at `[b"batch", epoch, hash_chain_index]`; writable.
6. `system_program` — read; creates the batch account.

**Instruction data**

```rust
struct CreateBatchIxData {
    hash_chain_index: u16,
    tx_viewing_pk: [u8; 33],
    salt: [u8; 16],
}
```

Requires a locked epoch within its settlement window, a valid hash-chain index, a nonempty chain, `executed == false`, and `batch_exists == false`.

The program copies the chain's hash and intent count from the epoch, records the crank, rent payer, `tx_viewing_pk`, and `salt`, allocates the batch at its full size, initializes an empty `utxos` vector, and sets `batch_exists = true` atomically.

### fill_batch

Appends escrow input records and optional payout outputs to the batch PDA within its allocated capacity. `None` marks a cancelled escrow; `Some(output)` marks an escrow to settle.

**Accounts**

1. `crank` — must match the batch's recorded crank signing address; signer.
2. `epoch` — read; must match `batch.epoch` and remain locked within its settlement window.
3. `batch` — writable; appends the input and output records.

**Instruction data**

```rust
struct FillBatchIxData {
    utxos: Vec<(InputUtxo, Option<TransactOutput>)>,
}
```

The SPP interface defines each record as:

```rust
struct InputUtxo {
    nullifier_hash: [u8; 32],
    tree_index: u8,
}

struct TransactOutput {
    utxo_hash: [u8; 32],
    owner_tag: OwnerTag,
    data: Option<Vec<u8>>,
}
```

`tree_index` selects the input tree account and its `TreeContext` in the SPP CPI. It is not a separate tree ID. The root indexes and proof are supplied to `execute_batch`; nullifier PDAs are supplied in its account list.

Requires an unexecuted chain and at least one entry. The total stored count must not exceed the chain's intent count or `N`, and ciphertext data must fit the reserved output format's capacity.

The program appends the entries and updates the stored count atomically. `execute_batch` verifies the cancellation markers and payout proofs.

### execute_batch

Settles one complete hash chain with a program-logic proof and two SPP proofs. Reads escrow inputs, cancellation markers, user payout outputs, and encryption parameters from the batch. Withdraws active escrow funds into the public vaults, executes the net pool operation, and deposits the payouts into SPP atomically.

**Accounts**

1. `payer` — pays transaction and SPP fees; signer, writable.
2. `config` — read; supplies the pool, mint, crank, and fixed intent fee.
3. `epoch` — writable; must match `batch.epoch`; records execution and resolved intent count.
4. `batch` — writable; supplies the escrow entries and payout encryption parameters; closed after execution.
5. `escrow_authority` — signs the SPP settlement CPI through the program.
6. `sol_vault` — writable; receives aggregate escrow SOL, funds net pool deposits, receives net pool withdrawals, and signs SPP payout deposits.
7. `token_vault` — writable; the SOL vault's configured pool-token account.
8. `crank` — writable; must match the batch's recorded crank signing address; receives intent fees.
9. `rent_payer` — writable; must match `batch.rent_payer`; receives batch rent.
10. `input_trees` — one SPP tree per `tree_contexts` entry, selected by each stored input's `tree_index`; writable when used for an active spend.
11. `input_nullifier_pdas` — one canonical nullifier PDA per stored entry; writable for active spends, read for cancellation checks.
12. `output_tree` — writable; receives the user payout outputs and supplies the payout address input's tree context.
13. `payout_nullifier_pda` — writable; canonical PDA for the program-derived zero-value payout address input.
14. `spp_accounts` — the SPP program, System Program, SOL/token interfaces, pool mint, and token program needed to construct both CPI account lists.
15. `stake_pool_accounts` — the configured program and pool, reserve, withdraw authority, fee accounts, and remaining accounts required by `DepositSol` or `WithdrawSol`.

**Instruction data**

```rust
struct ExecuteBatchIxData {
    /// Checks intent coverage, cancellations, balances, and payout encryption.
    settlement_proof: SettlementProof,
    tree_contexts: Vec<TreeContext>,
    payout_nullifier_root_index: u16,
    /// SPP withdraws escrow funds before the pool CPI.
    spend_proof: Option<SppProofData>,
    /// SPP deposits payouts after the pool CPI. These are separate SPP calls,
    /// so each needs a proof: the pool CPI cannot run inside SPP's transact.
    payout_proof: Option<SppProofData>,
    amounts: SettlementAmounts,
}

struct SppProofData {
    proof: TransactProof,
    private_tx_hash: [u8; 32],
}

struct SettlementAmounts {
    escrow_sol: u64,
    escrow_pool_tokens: u64,
    payout_sol: u64,
    payout_pool_tokens: u64,
}
```

The program builds both `SppTransactIxData` values from entries with `Some(output)`, the batch's `tx_viewing_pk` and `salt`, the epoch's settlement deadline as expiry, and the supplied SPP proof data. It selects the supported circuits and compact padding. Default-ring transaction data hashes are unset and `messages` is empty.

`tree_contexts` supplies authenticated roots for active spends and cancellation checks. The program selects trees used by active entries and remaps their input indexes for the withdrawal CPI. The payout CPI uses one fresh zero-value address input owned by `sol_vault`, with a seed derived from the batch PDA. The program derives its nullifier and uses `payout_nullifier_root_index` with `NO_UTXO_ROOT`.

Each `SppProofData` supplies an SPP proof and its `private_tx_hash`. Both are required when any entry has an output. If every entry is cancelled, both are absent and `amounts` is zero.

The program derives the withdrawal and deposit legs from `amounts` and directs them through the public vaults. It derives `DepositSol` from the SOL surplus or `WithdrawSol` from the pool-token surplus between escrow and payout totals. The other asset's deficit must match the pool operation's result. When both assets balance, the pool CPI is skipped. SOL deposits use the pool's manager fee account as the referral fee destination.

**Proof**

The settlement proof checks:

1. The original escrows reproduce the epoch's complete ordered hash chain and exact intent count. Each escrow has the expected owner, asset, and epoch-linked `IntentUtxoData`.
2. The stored entries cover those escrows exactly once and in order. Each input nullifier is derived from its escrow. An entry with `None` satisfies the authenticated spent check; an entry with `Some(output)` is included in the withdrawal CPI.
3. Each `Some(output)` has the opposite asset, the settlement amount determined by the agreed conversion and fee rules, `data_hash = 0`, and the escrow recipient's `owner_hash`.
4. Each stored output's ciphertexts encrypt the same complete payout UTXO to the escrow recipient and configured crank.
5. `amounts` equals the aggregate input and output amounts of active entries. These totals balance with the derived net pool operation using the current pool state, fixed fees, and the pool program's integer rounding. Address and compact-padding slots contribute zero value.
6. Both constructed SPP transactions match their supplied `private_tx_hash` values, public settlement legs, output hashes, and ciphertexts.

**Execution**

1. Require a locked epoch within its settlement window and an unexecuted chain. Check the batch's hash, intent count, and `utxos.len()` against the epoch entry, validate the configured pool and vault accounts, and verify the settlement proof and cancellation markers.
2. Record the vault balances. Build the SPP withdrawal instruction from active escrow inputs and invoke it with `escrow_authority` to withdraw aggregate SOL and pool tokens into the public vaults.
3. Derive and invoke the net `DepositSol` or `WithdrawSol` operation, signing with `sol_vault`. Skip the pool CPI when both assets balance.
4. Build the SPP payout instruction from the stored outputs and the program-derived zero-value address input. Invoke it with `sol_vault` to deposit the user payouts, then require both vault balances to equal their balances before execution.
5. Increment `resolved_intent_count` by the number of entries with outputs, set `executed = true` and `batch_exists = false`, pay that count times `intent_fee_lamports` to the recorded crank, subtract the payment from the epoch fee balance, and close the batch to its recorded rent payer.

When every entry is cancelled, steps 2–4 are skipped. Those cancellations were already counted in the epoch; execution closes the batch without incrementing the count or paying settlement fees.

The CPI account lists follow the [SPP interface](https://github.com/helius-labs/zolana/blob/92858c801026d78dde7b3203444050ca7a8adb34/program-libs/interface/src/instruction/instruction_data/transact.rs) and the stake pool's [instruction builders](https://github.com/solana-program/stake-pool/blob/8abfd17428728bc4c1ce181c238a3357bab82ad5/program/src/instruction.rs). A failed step rolls back the escrow spends, pool operation, payouts, fees, counters, and batch closure.
