# DvP

Delivery-versus-payment (DvP) settles a trade between two parties atomically: the seller's delivery leg and the buyer's payment leg move in the same transaction, or neither moves. Each party escrows its leg ahead of time as a shielded UTXO in the zolana shielded pool (SPP), and a designated settlement authority settles both legs once the trade is funded. Until settlement, either party can abort and recover its funds. The terms stay confidential to the seller, buyer, and settlement authority.

## Glossary

| Term | Type | Definition |
|---|---|---|
| Terms | Agreement | The `SwapDvp` fields the seller and buyer agree to. Each party accepts them by funding its leg. See [Data UTXOs](#data-utxos). |
| DvP ID | Hash | `hash(SwapDvp)`. Identifies the DvP and seeds its owner PDA. |
| DvP UTXO | Data UTXO | A data UTXO owned by the DvP PDA whose `data_hash` is the DvP ID. Holds no funds: its asset is SOL and its amount `0`. |
| Deposit UTXO | UTXO | A funded leg: a UTXO of the leg's mint owned by the DvP PDA, created by a plain SPP transfer from the funder. See [Data UTXOs](#data-utxos). |
| DvP PDA | PDA | `[b"dvp", creator, dvp_id]` under the DvP program. Owns the DvP's UTXOs, so only the DvP program can spend them. |
| DvP | Trade | The trade as a whole, from create to settle or abort. |
| Creator | Wallet (client) | Signs `create_dvp` and seeds the DvP PDA. A `Pubkey` in the terms; can be a single-use keypair. |
| Seller | Wallet (client) | Funds the delivery leg. Can reclaim its deposit or reject the DvP. |
| Buyer | Wallet (client) | Funds the payment leg. Can reclaim its deposit or reject the DvP. |
| Settlement authority | Server (venue, broker, or custodian backend) | Settles or cancels the DvP. Can also match the trade and create the DvP UTXO. |
| DvP program | Solana program | Verifies each instruction's proof and signs the SPP CPI as the DvP PDA. |
| SPP | Solana program | The zolana shielded pool. Holds the DvP's UTXOs and runs each `transact`. |

## Settlement flow

1. **Agree.** The seller and buyer agree on the terms (`SwapDvp`).
2. **Create.** The creator calls `create_dvp`, which creates the DvP address and the DvP UTXO and publishes the terms encrypted to the seller, buyer, and settlement authority. No funds move.
3. **Fund.** Each party decrypts its message and checks the terms against `dvp_id`, then transfers its leg to the DvP PDA in a plain SPP `transact`, creating a deposit UTXO. The seller funds the delivery leg and the buyer funds the payment leg. The transfer's recipient ciphertext is encrypted to the settlement authority's viewing key and its owner tag is the DvP PDA, so the settlement authority finds deposits by querying the indexer for the PDA.
4. **Settle.** Once both legs are funded and before expiry, the settlement authority settles. In one SPP transaction the delivery leg goes to the buyer's settlement destination, the payment leg to the seller's, and the DvP UTXO is spent.
5. **Abort.** Either party can reclaim its own deposit and leave the DvP open. Before settlement, a party can reject the DvP and the settlement authority can cancel it. Both refund each deposit to its refund destination and spend the DvP UTXO.

### Properties

**Functional**

1. Settle moves both legs in one SPP transaction, or neither moves.
2. Settle, cancel, and reject each spend the DvP UTXO. Its nullifier is inserted once, so at most one of them succeeds per DvP.
3. Each `(creator, dvp_id)` can be created once: the DvP address is a nullifier.
4. Reclaim works at any time, before or after expiry and whether or not the DvP is open. A deposit that lands after the DvP closes is reclaimed the same way.
5. Cancel and reject have no expiry check. Settle requires `earliest_settlement_timestamp <= current_timestamp <= expiry_timestamp`, with `current_timestamp` within `TIME_WINDOW` of the Clock.
6. The terms define the DvP UTXO and both settlement UTXOs; with the deposits, they also define the refunds. No step uses verifiable encryption.

**Privacy**

7. Public: the signer of each instruction (creator, seller, buyer, or settlement authority), `dvp_id` and the DvP PDA, the message view tags (seller, buyer, and settlement authority pubkeys), the owner tags of the settlement and refund outputs (the destinations' signing pubkeys), which instruction ran, and `current_timestamp` at settle.
8. Private: the mints, amounts, and price, `expiry_timestamp`, `earliest_settlement_timestamp`, `nonce`, `ref_string`, `blinding`, and the deposit amounts including any surplus.
9. The seller, buyer, and settlement authority read the terms from the create messages.
10. Anyone sees that a transfer to the DvP PDA happened. Only the settlement authority reads the deposit's asset and amount, from the funder's recipient ciphertext.
11. The transactions of one DvP are linkable through `dvp_id` and the DvP PDA. A funder is linked to the DvP through its signer and the PDA owner tag. A single-use creator keypair keeps the creator unlinked from its other activity.
12. A change output with no surplus is a zero-amount UTXO to its refund destination, which looks like a real output, so settle does not reveal whether a deposit was overfunded.

## Constants

| Constant | Value | Use |
| --- | --- | --- |
| `CREATE_TAG` | `"DVPC"` | Domain tag of the create `blinding_seed`. |
| `SETTLEMENT_TAG` | `"DVPS"` | Domain tag of the settle `blinding_seed`. |
| `ABORT_TAG` | `"DVPA"` | Domain tag of the cancel and reject `blinding_seed`. |
| `TIME_WINDOW` | `24` seconds (about 60 slots) | Maximum age of the `current_timestamp` a settle transaction proves against. |

Each four-letter tag is its ASCII bytes read as a big-endian 32-bit integer, as in zolana.

## Data UTXOs

### DvP UTXO

Owned by the DvP PDA `[b"dvp", creator, dvp_id]` with `nullifier_secret = 0`. Its `data_hash` is `dvp_id = hash(SwapDvp)` (`SwapDvp::hash` below) and its `utxo_data` is empty. Its asset is SOL and its amount is `0`. SPP spends only UTXOs with a nonzero asset, so the SOL asset lets settle, reject, and cancel spend it. It has no ciphertext; the parties recompute it from the terms.

`SwapDvp` is not stored in an account, and its plaintext is published encrypted in transaction messages. It keeps the field types and order of the reference program (`program/src/state/swap_dvp.rs`), except `bump`, which is removed: the PDA is derived from `dvp_id`, so its bump cannot be part of the hashed data. `delivery_token_program`, `payment_token_program`, `delivery_mint_authority`, and `payment_mint_authority` are removed. `seller_refund_destination`, `buyer_refund_destination`, `creator`, and `blinding` are added at the end. The settlement destinations are zolana `ShieldedAddress`es instead of `Pubkey`s. Fields are renamed: `user_a` -> `seller`, `user_b` -> `buyer`, `*_a` -> `delivery_*`, `*_b` -> `payment_*`.

Timestamps are non-negative: the proofs range-check them to 63 bits. The four destinations have Ed25519 or PDA signing keys.

```rust
pub struct SwapDvp {
    pub seller: Pubkey,
    pub buyer: Pubkey,
    pub delivery_mint: Pubkey,
    pub payment_mint: Pubkey,
    pub settlement_authority: Pubkey,
    pub delivery_amount: u64,
    pub payment_amount: u64,
    /// Settlement fails after this time. Cluster time (`Clock::unix_timestamp`).
    pub expiry_timestamp: i64,
    /// Distinguishes DvPs with otherwise identical terms.
    pub nonce: u64,
    /// Client reference, UTF-8 zero-padded. The DvP program does not read it.
    pub ref_string: [u8; 64],
    /// Receives the payment leg at settlement.
    pub seller_settlement_destination: ShieldedAddress,
    /// Receives the delivery leg at settlement.
    pub buyer_settlement_destination: ShieldedAddress,
    /// Settlement fails before this time, if set.
    pub earliest_settlement_timestamp: Option<i64>,
    /// Receives the seller's delivery refunds: settle surplus, cancel, and reject.
    pub seller_refund_destination: ShieldedAddress,
    /// Receives the buyer's payment refunds: settle surplus, cancel, and reject.
    pub buyer_refund_destination: ShieldedAddress,
    /// Signs `create_dvp` and seeds the DvP PDA. Can be a single-use keypair.
    pub creator: Pubkey,
    /// Random field element. Without it, guessed terms can be checked against `dvp_id`.
    pub blinding: [u8; 32],
}

impl SwapDvp {
    /// `dvp_id`. Nested, since Poseidon takes at most 12 inputs.
    pub fn hash(&self) -> [u8; 32] {
        let parties = Poseidon(
            solana_owner_identity(self.seller),
            solana_owner_identity(self.buyer),
            solana_owner_identity(self.settlement_authority),
            solana_owner_identity(self.creator),
        );
        let legs = Poseidon(
            hash_bytes(self.delivery_mint),
            hash_bytes(self.payment_mint),
            self.delivery_amount,
            self.payment_amount,
        );
        // An unset earliest settlement timestamp hashes as (0, 0).
        let (has_earliest, earliest) = match self.earliest_settlement_timestamp {
            Some(timestamp) => (1, timestamp),
            None => (0, 0),
        };
        let schedule = Poseidon(self.expiry_timestamp, has_earliest, earliest, self.nonce);
        let destinations = Poseidon(
            self.seller_settlement_destination.compressed_address().hash(),
            self.buyer_settlement_destination.compressed_address().hash(),
            self.seller_refund_destination.compressed_address().hash(),
            self.buyer_refund_destination.compressed_address().hash(),
        );
        Poseidon(
            parties,
            legs,
            schedule,
            destinations,
            hash_bytes(self.ref_string),
            self.blinding,
        )
    }
}
```

### Deposit UTXO

Owned by the DvP PDA with `nullifier_secret = 0`, with the leg's mint as asset, at least the leg's amount, and `data_hash = 0`. SPP requires a signing owner for an output with a nonzero `data_hash`, and the PDA does not sign a plain transfer. The PDA is unique per DvP, so a deposit's owner identifies its DvP.

## Instructions

| # | Instruction | Description | Access |
| --- | --- | --- | --- |
| 1 | `create_dvp` | Creates the DvP address and the DvP UTXO, and publishes the encrypted terms. No funds move. | Creator signs |
| 2 | `reclaim_dvp` | Returns one of the signer's deposit UTXOs to the signer. The DvP stays open. | Seller or buyer signs |
| 3 | `settle_dvp` | Spends the DvP UTXO and both deposits; pays each leg to its settlement destination and the surplus to the refund destinations. | Settlement authority signs |
| 4 | `cancel_dvp` | Spends the DvP UTXO and any deposits, and refunds each deposit to its refund destination. | Settlement authority signs |
| 5 | `reject_dvp` | Same as `cancel_dvp`. | Seller or buyer signs |

### 1. create_dvp

Creates the DvP address and the DvP UTXO, and publishes the terms encrypted to the seller, buyer, and settlement authority. No funds move.

**Accounts**

1. `payer` — pays transaction fees; signer, writable.
2. `creator` — `SwapDvp.creator`; signer.
3. `spp_accounts` — the SPP `transact` account list (program, output tree, input trees, and nullifier PDAs), ending in `dvp_authority`, the DvP PDA `[b"dvp", creator, dvp_id]`: owner of the DvP address and the DvP UTXO, and the owner signer the program signs for in the SPP CPI.

**Instruction data**

```rust
struct CreateDvpIxData {
    dvp_id: [u8; 32],
    /// Canonical bump of `dvp_authority`; the program rejects any other.
    bump: u8,
    create_proof: CreateDvpProof,
    spp: SppTransactIxData,
}
```

The create `transact` has shape `1 × 2`. The input is an SPP address (`AddressDomain`) in tree `0`, owned by `dvp_authority` with address seed `dvp_id`. Its nullifier is the address, and an address exists once per owner, seed, and tree, so each `(creator, dvp_id)` can be created once. Output `0` is the DvP UTXO. Output `1` is compact padding, since SPP has no `1 × 1` shape; it has no ciphertext slot.

`spp.messages[0..3]` are the plaintext `SwapDvp` encrypted to the seller, buyer, and settlement authority, in that order, one `MessageData { view_tag, data }` each. The creator knows their viewing public keys. Each `view_tag` is the recipient's pubkey, the way default-ring outputs are tagged by their owner pubkey, so a recipient finds its message by querying the indexer for its own pubkey. Encryption follows the zolana AES key derivation. Message `j` uses ciphertext slot `1 + j`, after the DvP UTXO's, and its `data` is the bare ciphertext.

**UTXO data**

The DvP UTXO's `utxo_data` is empty and its `data_hash` is `dvp_id = hash(SwapDvp)`. Its asset is SOL and its amount is `0`.

**Proof**

The create proof checks:

1. `dvp_id = hash(SwapDvp)` for the `SwapDvp` witness, and `SwapDvp.creator` equals the public `creator`.
2. Input `0` is the address in tree `0` owned by `dvp_authority` with seed `dvp_id`.
3. Output `0` is owned by `dvp_authority` with `nullifier_secret = 0`, has asset SOL and amount `0`, and its `data_hash` equals `dvp_id`.
4. `delivery_amount > 0`, `payment_amount > 0`, `delivery_mint != payment_mint`, the timestamps are non-negative, and `earliest_settlement_timestamp <= expiry_timestamp` if set.
5. The SPP `blinding_seed` is `Poseidon(CREATE_TAG, SwapDvp.blinding)`. SPP derives each output blinding from the `blinding_seed` and the first nullifier. The first nullifier is the DvP address, so the terms fix the DvP UTXO's blinding.
6. The address nullifier and output hash match the submitted SPP transaction. The proof checks the same `private_tx_hash` passed to SPP.

The program derives `dvp_authority` from `creator`, `dvp_id`, and `bump`, computes the proof's public inputs from `creator`, `dvp_id`, and the SPP instruction data, verifies the proof, and invokes SPP signing as `dvp_authority`.

The proof does not check the messages. A recipient decrypts its message and checks `hash(SwapDvp) == dvp_id` before acting. Create does not check `expiry_timestamp` against the Clock: a DvP created with a past expiry cannot settle.

### 2. reclaim_dvp

Returns one deposit UTXO of the signer's leg to the signer, whatever its amount. The DvP UTXO and the other leg are untouched, so the leg can be funded again.

**Accounts**

1. `payer` — pays transaction fees; signer, writable.
2. `signer` — `SwapDvp.seller` or `SwapDvp.buyer`; signer. Its identity selects the leg: the seller reclaims a delivery deposit, the buyer a payment deposit.
3. `spp_accounts` — the SPP `transact` account list, ending in `dvp_authority`, the DvP PDA `[b"dvp", creator, dvp_id]`: owner of the deposit UTXO, and the owner signer the program signs for in the SPP CPI.

**Instruction data**

```rust
struct ReclaimDvpIxData {
    creator: Pubkey,
    dvp_id: [u8; 32],
    /// Canonical bump of `dvp_authority`; the program rejects any other.
    bump: u8,
    reclaim_proof: ReclaimDvpProof,
    spp: SppTransactIxData,
}
```

The reclaim `transact` has shape `1 × 2`: the deposit UTXO in, the refund UTXO and compact padding out.

**Proof**

The reclaim proof checks:

1. `dvp_id = hash(SwapDvp)` for the `SwapDvp` witness, and `SwapDvp.creator` equals the public `creator`.
2. The public `signer` equals `SwapDvp.seller` or `SwapDvp.buyer`. The leg mint is `delivery_mint` for the seller and `payment_mint` for the buyer.
3. Input `0` is owned by `dvp_authority` with `nullifier_secret = 0`, has `data_hash = 0`, and its asset is the signer's leg mint. Its amount is not checked.
4. Output `0` has the input's asset and amount and `data_hash = 0`. The signer chooses its owner.
5. The input nullifier and output hash match the submitted SPP transaction. The proof checks the same `private_tx_hash` passed to SPP.

The program derives `dvp_authority` from `creator`, `dvp_id`, and `bump`, verifies the proof, and invokes SPP signing as `dvp_authority`.

Reclaim has no expiry check and does not check that the DvP is open. A deposit that lands after settle or abort is reclaimed the same way.

### 3. settle_dvp

Spends the DvP UTXO and both deposit UTXOs. Pays the delivery leg to `buyer_settlement_destination`, the payment leg to `seller_settlement_destination`, and any surplus to the refund destinations. Spending the DvP UTXO closes the DvP.

**Accounts**

1. `payer` — pays transaction fees; signer, writable.
2. `settlement_authority` — `SwapDvp.settlement_authority`; signer.
3. `spp_accounts` — the SPP `transact` account list, ending in `dvp_authority`, the DvP PDA `[b"dvp", creator, dvp_id]`: owner of the DvP UTXO and the deposit UTXOs, and the owner signer the program signs for in the SPP CPI.

**Instruction data**

```rust
struct SettleDvpIxData {
    creator: Pubkey,
    dvp_id: [u8; 32],
    /// Canonical bump of `dvp_authority`; the program rejects any other.
    bump: u8,
    /// Time the proof checks the terms against. The program requires it to be
    /// within `TIME_WINDOW` of the Clock.
    current_timestamp: i64,
    settle_proof: SettleDvpProof,
    spp: SppTransactIxData,
}
```

The settle `transact` has shape `3 × 4`. Inputs: the DvP UTXO (slot `0`), the delivery deposit (slot `1`), and the payment deposit (slot `2`). Outputs: the delivery settlement UTXO (slot `0`), the payment settlement UTXO (slot `1`), the delivery change (slot `2`), and the payment change (slot `3`).

**Proof**

The settle proof checks:

1. `dvp_id = hash(SwapDvp)` for the `SwapDvp` witness, and `SwapDvp.creator` equals the public `creator`.
2. The public `settlement_authority` equals `SwapDvp.settlement_authority`.
3. `current_timestamp <= expiry_timestamp`, and `earliest_settlement_timestamp <= current_timestamp` if set. The terms' timestamps stay private.
4. Input `0` is the DvP UTXO: owned by `dvp_authority` with `nullifier_secret = 0`, asset SOL, amount `0`, and `data_hash = dvp_id`.
5. Input `1` is owned by `dvp_authority` with `nullifier_secret = 0`, `data_hash = 0`, asset `delivery_mint`, and amount at least `delivery_amount`. Input `2` is the same with `payment_mint` and `payment_amount`.
6. Output `0` has owner `buyer_settlement_destination`, asset `delivery_mint`, amount `delivery_amount`, and `data_hash = 0`. Output `1` has owner `seller_settlement_destination`, asset `payment_mint`, amount `payment_amount`, and `data_hash = 0`.
7. Output `2` has owner `seller_refund_destination`, asset `delivery_mint`, and amount `input_1.amount - delivery_amount`. Output `3` has owner `buyer_refund_destination`, asset `payment_mint`, and amount `input_2.amount - payment_amount`. A change output with no surplus has amount `0`.
8. The SPP `blinding_seed` is `Poseidon(SETTLEMENT_TAG, SwapDvp.blinding)`. The first nullifier is the DvP UTXO's, so the terms fix the blindings of the settlement and change outputs.
9. The input nullifiers and output hashes match the submitted SPP transaction. The proof checks the same `private_tx_hash` passed to SPP.

The program checks `0 <= current_timestamp` and `now - TIME_WINDOW <= current_timestamp <= now` against the Clock, derives `dvp_authority` from `creator`, `dvp_id`, and `bump`, verifies the proof, and invokes SPP signing as `dvp_authority`.

A leg funded with less than its amount is reclaimed and funded again.

Because `current_timestamp` may lag the Clock by up to `TIME_WINDOW`, a settle can land up to `TIME_WINDOW` after `expiry_timestamp`. Parties set the expiry with that margin.

### 4. cancel_dvp

Spends the DvP UTXO and up to one deposit per leg, and refunds each deposit in full to its refund destination. Spending the DvP UTXO closes the DvP.

**Accounts**

1. `payer` — pays transaction fees; signer, writable.
2. `settlement_authority` — `SwapDvp.settlement_authority`; signer.
3. `spp_accounts` — the SPP `transact` account list, ending in `dvp_authority`, the DvP PDA `[b"dvp", creator, dvp_id]`: owner of the DvP UTXO and the deposit UTXOs, and the owner signer the program signs for in the SPP CPI.

**Instruction data**

```rust
struct CancelDvpIxData {
    creator: Pubkey,
    dvp_id: [u8; 32],
    /// Canonical bump of `dvp_authority`; the program rejects any other.
    bump: u8,
    cancel_proof: AbortDvpProof,
    spp: SppTransactIxData,
}
```

The cancel `transact` has shape `3 × 2`. Inputs: the DvP UTXO (slot `0`), then the deposits in slots `1` and `2`, padding last, as SPP requires. A private leg-order bit says which leg is in slot `1`, so the transaction does not show which legs were funded. Outputs: the delivery refund (slot `0`) and the payment refund (slot `1`).

**Proof**

The abort proof checks:

1. `dvp_id = hash(SwapDvp)` for the `SwapDvp` witness, and `SwapDvp.creator` equals the public `creator`.
2. The public signer equals `SwapDvp.settlement_authority`.
3. Input `0` is the DvP UTXO: owned by `dvp_authority` with `nullifier_secret = 0`, asset SOL, amount `0`, and `data_hash = dvp_id`.
4. Inputs `1` and `2` are the delivery and payment deposits, in the order the leg-order bit gives, with padding last. A deposit is owned by `dvp_authority` with `nullifier_secret = 0`, has `data_hash = 0`, and has its leg's mint. Deposit amounts are not checked.
5. Output `0` refunds the delivery deposit: owner `seller_refund_destination`, the deposit's asset and amount, and `data_hash = 0`. Output `1` refunds the payment deposit to `buyer_refund_destination` the same way. A leg without a deposit gets a refund of amount `0`.
6. The SPP `blinding_seed` is `Poseidon(ABORT_TAG, SwapDvp.blinding)`. The first nullifier is the DvP UTXO's, so the terms fix the refund blindings.
7. The input nullifiers and output hashes match the submitted SPP transaction. The proof checks the same `private_tx_hash` passed to SPP.

The program derives `dvp_authority` from `creator`, `dvp_id`, and `bump`, verifies the proof, and invokes SPP signing as `dvp_authority`.

Cancel has no expiry check. A leg with more than one deposit refunds one at cancel; the others are reclaimed.

### 5. reject_dvp

Same as `cancel_dvp`, signed by a party instead of the settlement authority.

**Accounts**

1. `payer` — pays transaction fees; signer, writable.
2. `signer` — `SwapDvp.seller` or `SwapDvp.buyer`; signer.
3. `spp_accounts` — the SPP `transact` account list, ending in `dvp_authority`, the DvP PDA `[b"dvp", creator, dvp_id]`: owner of the DvP UTXO and the deposit UTXOs, and the owner signer the program signs for in the SPP CPI.

**Instruction data**

```rust
struct RejectDvpIxData {
    creator: Pubkey,
    dvp_id: [u8; 32],
    /// Canonical bump of `dvp_authority`; the program rejects any other.
    bump: u8,
    reject_proof: AbortDvpProof,
    spp: SppTransactIxData,
}
```

The reject `transact` has the `cancel_dvp` shape.

**Proof**

The abort proof of `cancel_dvp`, with check 2 replaced: the public signer equals `SwapDvp.seller` or `SwapDvp.buyer`.

## Limitations

### Malformed deposits are lost

Funding is a plain SPP transfer that the DvP program does not check. A UTXO sent to the DvP PDA in an asset other than `delivery_mint` or `payment_mint` matches no reclaim, settle, reject, or cancel proof, so no one can spend it.

### Confidential, not anonymous

The DvP hides the mints, amounts, and time bounds. It does not hide the participants. As in the zolana default ring, the create transaction tags its messages with the seller, buyer, and settlement authority pubkeys, and the settle, cancel, and reject outputs are tagged with the destinations' signing pubkeys.

An anonymous version changes two things:

1. The proofs enforce view tags instead of pubkeys for the create messages and for the settlement, change, and refund outputs.
2. A relayer submits every transaction and pays its fees. The seller, buyer, creator, and settlement authority authorize inside the proofs instead of signing the transaction.
