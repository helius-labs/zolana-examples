# Minimal multisig

This program lets an MPC multisig propose and approve a shielded transaction, then execute it with a fresh proof once approvals are complete. The shielded UTXOs are owned by a PDA derived from the multisig's Ed25519 public key under this program, allowing the program to sign default-ring settlement through CPI once it verifies the multisig's approval.

## Glossary

| Term | Type | Definition |
|---|---|---|
| Shielded transaction inputs | UTXOs | The UTXOs selected for spending, owned by the multisig's derived PDA. |

## Settlement flow

1. **Proposer (client + program) — Propose.** Fix the shielded transaction's inputs, outputs, ciphertexts and expiry. Create a `ProposedTransaction` account containing its authorization commitment, multisig identity, proposal expiry, rent recipient and abort authorities.
2. **MPC signers (client) — Approve.** Verify the proposed transaction against the `ProposedTransaction` account and sign a detached approval binding the account identity and its contents.
3. **Executor (client + program) — Execute.** Generate a fresh default-ring proof and submit the transaction with the multisig approval. The program verifies the approval and commitment, then invokes SPP signing as the multisig's owner PDA. On success, it atomically closes the `ProposedTransaction` account and refunds rent to the recorded recipient.

### Properties

## Constants

## Accounts

### ProposedTransaction

PDA seeds: `[b"proposed_transaction", multisig_pubkey, authorization_commitment]` under this program, using the canonical bump.

```rust
pub struct ProposedTransaction {
    /// SHA-256(private_tx_hash || external_data_hash), binding the payment
    /// contents while allowing Merkle roots, witnesses and proof bytes to change.
    pub authorization_commitment: [u8; 32],
    pub multisig_pubkey: Pubkey,
    /// Execution deadline in Unix seconds, checked against the Solana Clock.
    pub expiry_unix_ts: u64,
    pub rent_recipient: Pubkey,
    pub abort_authorities: Vec<Pubkey>,
}
```

### CancelledTransaction

PDA seeds: `[b"cancelled_transaction", proposed_transaction]` under this program, using the canonical bump.

```rust
pub struct CancelledTransaction {
    /// Retain the cancellation record through this deadline.
    pub expiry_unix_ts: u64,
    /// Receives rent when the cancellation account closes.
    pub rent_recipient: Pubkey,
}
```

## Instructions

| # | Instruction | Description | Access |
| --- | --- | --- | --- |
| 1 | `create_proposed_transaction` | Creates a `ProposedTransaction` account. No funds move. | Proposer signs |
| 2 | `execute_proposed_transaction` | Settles the approved transaction through SPP and closes the account. | Executor submits the multisig's detached approval |
| 3 | `cancel_proposed_transaction` | Cancels the proposal, closes its account and refunds rent to the recorded recipient. | An abort authority signs, or anyone after expiry |
| 4 | `close_cancelled_transaction` | Closes an expired cancellation PDA and refunds rent to its recorded recipient. | Anyone after expiry |

### 1. create_proposed_transaction

Creates an immutable `ProposedTransaction` account for the multisig to inspect and approve. No shielded funds move, and the multisig does not sign this instruction.

**Accounts**

1. `payer` — pays transaction fees and funds the account's rent exemption; signer, writable.
2. `proposed_transaction` — the PDA `[b"proposed_transaction", multisig_pubkey, authorization_commitment]` under this program; writable.
3. `system_program` — the Solana System Program.

**Instruction data**

```rust
pub struct CreateProposedTransactionIxData {
    /// SHA-256(private_tx_hash || external_data_hash).
    pub authorization_commitment: [u8; 32],
    /// Ed25519 group public key that authorizes execution.
    pub multisig_pubkey: Pubkey,
    /// Execution deadline in Unix seconds.
    pub expiry_unix_ts: u64,
    /// Receives rent when the account closes.
    pub rent_recipient: Pubkey,
    /// Any listed authority may cancel the proposal.
    pub abort_authorities: Vec<Pubkey>,
}
```

The program checks:

1. `payer` is a signer.
2. `proposed_transaction` matches the PDA derived from the instruction's `multisig_pubkey` and `authorization_commitment` using the canonical bump, and is uninitialized.
3. `system_program` is the System Program.
4. `expiry_unix_ts` is strictly greater than the current Solana Clock Unix timestamp. A negative Clock timestamp is rejected.
5. `abort_authorities` contains no duplicates.

The program creates the PDA through a signed System Program CPI, allocates enough space for the `ProposedTransaction`, funds its rent exemption from `payer`, assigns ownership to this program and stores the instruction data. Its contents cannot be updated.

The owner PDA is derived as `[b"multisig", multisig_pubkey]` under this program. It owns the shielded inputs and signs the SPP CPI during execution; creating a proposal does not create or fund that PDA.

**Client verification**

The proposer shares the transaction contents needed to reproduce the commitment and verify the inputs, outputs and ciphertexts. These contents are not stored in the account.

Before approving, each signer checks the account's program ownership, multisig key, transaction commitment, proposal expiry, rent recipient and abort authorities. The detached approval binds the program and deployment, account address and all stored fields. Roots, witnesses and proof bytes are excluded so they can be refreshed.

### 2. execute_proposed_transaction

Executes the approved shielded transaction through the default ring and closes the `ProposedTransaction` account. The executor supplies a fresh proof; the multisig does not sign the outer Solana transaction.

**Accounts**

1. `payer` — pays transaction fees; signer, writable.
2. `proposed_transaction` — the `ProposedTransaction` PDA; writable.
3. `multisig_authority` — the owner PDA `[b"multisig", multisig_pubkey]` under this program.
4. `cancellation` — the cancellation PDA derived from `proposed_transaction`; must not contain a cancellation record.
5. `rent_recipient` — the recipient recorded in `ProposedTransaction`; writable.
6. `instructions_sysvar` — the Solana Instructions sysvar.
7. `spp_accounts` — the accounts required by the default-ring SPP `transact`, including the SPP program, trees, nullifier accounts and settlement accounts.

**Instruction data**

```rust
pub struct ExecuteProposedTransactionIxData {
    /// Index of the preceding Ed25519 verification instruction.
    pub approval_instruction_index: u16,
    /// Default-ring transaction, including its fresh proof.
    pub spp: SppTransactIxData,
}
```

**Approval**

The executor includes an Ed25519 verification instruction before this instruction. It verifies the multisig's signature over:

```text
SHA-256(
    APPROVAL_DOMAIN
    || deployment_id
    || program_id
    || proposed_transaction_address
    || serialize(ProposedTransaction)
)
```

`APPROVAL_DOMAIN` is a versioned domain separator. The account fields use the program's canonical serialization. The approval binds the authorization commitment, multisig key, proposal expiry, rent recipient and ordered abort authorities.

The Ed25519 instruction must contain exactly one verification, with its public key, signature and message stored in that instruction's own data.

**Checks**

The program checks:

1. `payer` is a signer, and `instructions_sysvar` is the Instructions sysvar.
2. `proposed_transaction` belongs to this program, has the expected account type and matches its canonical PDA.
3. The cancellation account matches the expected PDA and contains no cancellation record.
4. The Clock timestamp is non-negative and does not exceed the proposal's `expiry_unix_ts`.
5. `rent_recipient` matches the recorded recipient.
6. The referenced instruction precedes this instruction, targets the Ed25519 verification program and verifies the recorded `multisig_pubkey` over the exact approval message.
7. The submitted SPP instruction uses the default ring.
8. Recomputing the canonical SPP external-data hash and `SHA-256(private_tx_hash || external_data_hash)` produces the recorded `authorization_commitment`.
9. `multisig_authority` matches the canonical owner PDA.

**Settlement**

The program invokes the canonical SPP program with the submitted transaction, signing only as `multisig_authority` through `invoke_signed`. SPP verifies the proof, ownership, root validity, nullifiers, balances and transaction expiry.

Roots, witnesses and proof bytes may be refreshed without changing the approved commitment. The committed payment contents remain fixed.

After successful settlement, the program closes `ProposedTransaction` and transfers its lamports to `rent_recipient`. Settlement and closure are atomic. If any check or the SPP invocation fails, the proposal remains open.

Successful execution needs no cancellation record: SPP's input nullifiers prevent the approved transaction from spending again.

### 3. cancel_proposed_transaction

Cancels a proposal, closes its account and refunds rent. Before expiry, it creates a cancellation PDA so recreating the proposal cannot revive an existing approval. After expiry, no cancellation record is needed.

**Accounts**

1. `payer` — pays transaction fees and funds the cancellation account when needed; signer, writable.
2. `proposed_transaction` — the `ProposedTransaction` PDA; writable.
3. `cancellation` — the PDA `[b"cancelled_transaction", proposed_transaction]` under this program; writable.
4. `abort_authority` — a recorded abort authority; signer when cancelling an unexpired proposal, omitted otherwise.
5. `rent_recipient` — the proposal's recorded rent recipient; writable.
6. `system_program` — the Solana System Program.

**Instruction data**

None.

**Checks**

The program checks:

1. `payer` is a signer, and `system_program` is the System Program.
2. `proposed_transaction` belongs to this program, has the expected account type and matches its canonical PDA.
3. `cancellation` matches its canonical PDA.
4. The Clock timestamp is non-negative.
5. `rent_recipient` matches the proposal's recorded recipient.
6. If the Clock timestamp does not exceed the proposal expiry, `abort_authority` is a signer listed in `abort_authorities`.

**Unexpired proposal**

When the Clock timestamp does not exceed `expiry_unix_ts`, the program creates the cancellation PDA through a signed System Program CPI, funded by `payer`. It copies the proposal's expiry and rent recipient into `CancelledTransaction`, then closes `ProposedTransaction` and refunds its lamports to `rent_recipient`.

If a valid cancellation record already exists, the program retains it. It does not overwrite its expiry or rent recipient. The existing record must expire no earlier than the proposal being cancelled; otherwise the instruction fails.

Recording cancellation and closing the proposal are atomic. Execution rejects any proposal at this address while the cancellation record exists. The cancellation account's contents cannot be updated.

**Expired proposal**

When the Clock timestamp is strictly greater than `expiry_unix_ts`, anyone may close the proposal and refund rent to its recorded recipient. No cancellation PDA is created: the expired approval cannot authorize execution.

Any existing cancellation record is left unchanged. Its cleanup is handled separately by `close_cancelled_transaction`.

### 4. close_cancelled_transaction

Closes an expired `CancelledTransaction` account and refunds rent to its recorded recipient. Anyone may submit this instruction; no multisig approval or abort-authority signature is required.

**Accounts**

1. `payer` — pays transaction fees; signer.
2. `proposed_transaction` — the original proposal address, used to derive the cancellation PDA. It need not contain a live proposal account.
3. `cancellation` — the PDA `[b"cancelled_transaction", proposed_transaction]` under this program; writable.
4. `rent_recipient` — the recipient recorded in `CancelledTransaction`; writable.

**Instruction data**

None.

**Checks**

The program checks:

1. `payer` is a signer.
2. `cancellation` belongs to this program, has the `CancelledTransaction` account type and matches its canonical PDA derived from `proposed_transaction`.
3. The Clock timestamp is non-negative and strictly greater than the expiry recorded in `CancelledTransaction`.
4. `rent_recipient` matches the recipient recorded in `CancelledTransaction`.

The program closes `CancelledTransaction` and transfers its lamports to `rent_recipient`.

The expiry is read only from the cancellation record, independently of any proposal recreated at the original address. Removing the record cannot revive the cancelled approval because its proposal expiry has passed. This instruction does not close or modify a recreated proposal.

## Limitations
