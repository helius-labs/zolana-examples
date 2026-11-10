# Swap Program

The swap program settles a confidential swap between a maker and a taker the maker chooses. The two
agree a price out of band, and the maker commits an order that locks the
funds it is selling as a shielded UTXO in the Solana Privacy Program (SPP). The taker indexes the order
made with the make transaction, encrypted so only the taker and the maker can read its private terms.

The taker takes the order before it expires, receiving the maker's funds and paying the agreed amount
in return; if the taker declines, the maker reclaims the order UTXO after expiry. The maker either
designates the taker in the order, so that no one else can settle it, or leaves the order open to
anyone who holds its opening.

The taker learns the order and may decline to take it, but it cannot take the locked funds: the
program alone can move the order UTXO, and only by settling the committed order or refunding the maker
after expiry. Amounts and the price stay private. That a swap was made and later taken or
cancelled is public; taking reveals the taker and cancelling reveals the maker, who signs it.

The swap program is an SPP ZK program: it verifies a small proof of its own swap rules and delegates
the confidential transfer to SPP. It stores no state and owns no accounts.

This document specifies the swap's privacy model, the order terms, the program's instructions, and
its circuits.

## Flow

```mermaid
sequenceDiagram
    participant Maker
    participant Taker
    participant Swap as Swap Program
    participant SPP as Privacy Program

    Note over Maker: 1. Make the order (make)
    Maker->>Swap: make (make proof + SPP transact)
    Swap->>SPP: CPI transact -> change + order UTXO (swap utxo_data) + marker message
    Note over SPP: spend maker source UTXO, append change + order UTXO <br> source_asset_id public, source_amount + order terms private in utxo_data <br> order UTXO ciphertext encrypted to taker viewing pubkey; marker message tagged to taker
    Note over Taker,SPP: taker wallet sync finds the marker message (its tag), <br> decrypts the matching order UTXO slot -> order UTXO hash preimage (terms + blinding)
    Note over Maker: maker can decrypt the order UTXO slot via the tx viewing key it holds

    Note over Taker: 2a. Take, before expiry (Taker holds the order UTXO hash preimage)
    Taker->>Swap: take (take proof + SPP transact)
    Swap->>SPP: CPI transact: order UTXO + Taker destination UTXO -> source UTXO to Taker + destination UTXO to maker
    Note over SPP: destination blinding derived from the order UTXO blinding and the first nullifier; order UTXO consumed

    Note over Maker: 2b. Cancel, after expiry
    Maker->>Swap: cancel (maker-signed, cancel proof + SPP transact)
    Swap->>SPP: CPI transact: order UTXO -> source UTXO back to the maker
    Note over SPP: order UTXO consumed
```

## Table of Contents

- [Glossary](#glossary)
- [Privacy Model](#privacy-model)
- [Accounts](#accounts)
- [Order Terms](#order-terms)
- [Instructions](#instructions)
  - [make](#make)
  - [take](#take)
  - [cancel](#cancel)
- [Circuits](#circuits)
  - [Make circuit](#make-circuit)
  - [Take circuit](#take-circuit)
  - [Cancel circuit](#cancel-circuit)

## Glossary

Types used in this document. Shared SPP types are defined in [spec.md](../../docs/spec.md#glossary).

| Type | Encoding | Definition |
| --- | --- | --- |
| `Address` | `[u8; 32]` | Solana account address. |
| `asset_id` | `u64` | Asset identifier in UTXOs; `1` is SOL, each SPL mint `≥ 2`. The mint→`asset_id` map is the SPP `Asset registry` PDA. See [spec.md](../../docs/spec.md#glossary). |
| `CompressedShieldedAddress` | `[u8; 65]` | `(owner_hash [u8;32], viewing_pk tagged X25519[33])`. See [spec.md](../../docs/spec.md#shielded-address). |
| `order-authority PDA` | `Address` | The maker's program address, `find_program_address([b"order_authority", maker], program_id)`: one per maker. It owns the maker's order UTXOs, so a maker finds them by deriving it. |
| `order UTXO` | — | The SPP [UTXO](../../docs/spec.md#utxo) holding the source funds: `asset = source_asset_id`, `amount = source_amount`, `owner = the maker's order-authority PDA`, nullifier secret `= 0`, `utxo_data = order terms`. The make proof enforces the owner, so the UTXO is spendable only by the swap program. See [Order Terms](#order-terms). |
| `marker message` | — | The SPP `transact` message (`view_tag`, `data`) `make` appends as the taker's discovery tag: `view_tag` is the taker's confidential view tag, `data` a plaintext [`MarkerData`](#make) the program writes. Committed by the SPP proof through the transact's external-data hash; the `view_tag` is unenforced: a wrong tag only means the taker does not index the trade. |
| `MarkerData` | Borsh | `{ order_utxo_hash: [u8;32], maker_pubkey: [u8;32] }`, the plaintext `make` writes into the transact's single marker message. `order_utxo_hash` locates the order UTXO slot; `maker_pubkey` is the `make` signer's Solana pubkey, which the taker resolves to the maker's registered shielded address via the user registry. See [make](#make). |
| `Order terms` | — | The fields committed in the order UTXO's `utxo_data` (record tag `0x02`), hashed into the order UTXO `utxo_hash` via `data_hash`: `destination_asset_id`, `destination_amount`, `maker_address`, `expiry`, `taker_pk_fe`. See [Order Terms](#order-terms). |
| `private_tx_hash` | `[u8; 32]` | Commitment to the SPP `transact` a swap proof authorizes: the link between a swap proof and the SPP transaction. See [spec.md](../../docs/spec.md#zk-program-interface). |
| `MakeProof` / `TakeProof` / `CancelProof` | `[u8; 128]` | Standard Groth16 proofs verified by the swap program, each committing the transaction via `private_tx_hash`. |
| `TransactIxData` | — | SPP `transact` instruction data: the SPP proof, input nullifiers, output UTXO hashes, ciphertexts, and routing. See [spec.md](../../docs/spec.md#transact). |
| `solana_owner_identity` | fn | The owner pk field of a Solana pubkey: `hash_bytes` of the key tagged as a Solana key. An owner hash is `Poseidon(owner_pk_field, nullifier_pk)`; the program computes it from a signer so the proof can compare it against a committed hash. |

## Privacy Model

What is public and what is private. The confidentiality is inherited from the SPP confidential
ring; the swap program does not try to hide which action ran.

- **Public:** the maker's Solana signer pubkey, written into the marker at make (the taker resolves
  the maker's registered shielded address from it via the user registry) and revealed again at
  cancel, which the maker signs; which swap instruction ran; `source_asset_id` at make and both
  `source_asset_id` and `destination_asset_id` at resolve (`asset_id`s are SPP public inputs); the
  maker whose order a take settles, since the take names the maker and its order-authority PDA
  (the swap is confidential, not anonymous: its parties may be public, its amounts and price are
  not); the
  order UTXO hash at
  make; the order `expiry`, revealed at resolve so the program can check it against the Clock; each
  transaction's SPP output UTXO hashes and ciphertexts; the fee payer's identity on `take`, since it
  signs the transaction.
- **Private:** `price`, `source_amount`, `destination_amount`, and the aggregate volume per asset.
  These live only inside confidential UTXOs and the order UTXO `utxo_data`.
  The taker's identity stays private at make and on cancel, which the maker signs. Whether an
  order designates a taker, and which one, is private: `taker_pk_fe` is an order term inside
  `utxo_data`.
- **Unlinkable:** SPP hides the link between a created UTXO and its later spend, so when a maker
  has several open orders an observer cannot tell which of them a take or cancel spent; it does
  see whose order it was.

## Accounts

The swap program owns no accounts: the order and its funds live in the order UTXO, a leaf in the
SPP trees, moved by CPI. The
taker's spread (the gap
between the `source_amount` it receives and the `destination_amount` it pays) is its only
compensation.

## Order Terms

The order UTXO holds the order terms and funds. `make` writes the
order terms into the order UTXO's `utxo_data` (record tag `0x02`), and SPP commits them into the
order UTXO `utxo_hash` through `data_hash` (committed unchecked, interpreted by the swap circuit — see
[spec.md](../../docs/spec.md#utxo)):

```text
order_terms = (
    destination_asset_id,   // asset_id; private until resolve
    destination_amount,     // private; price = destination_amount / source_amount is implicit and private
    maker_address,           // the maker's CompressedShieldedAddress, resolved by the taker from the user registry: receives destination on take and source on cancel
    expiry,                 // unix seconds; revealed at resolve and checked against the Clock by the program
    taker_pk_fe,            // the designated taker's owner pk field, or 0 for an open order; checked by the take circuit
)
data_hash = Poseidon(order_terms)        // gnark: enters the order UTXO utxo_hash directly; Xark: see below
```

Xark: the order UTXO is the program SDK's `DataUtxo<OrderTerms>`, so its data hash is the SDK
state hash `Poseidon(DATA, Poseidon(discriminator, hash_bytes(destination_mint),
destination_amount, maker_owner_hash, expiry, hash_bytes(taker_pk_fe)))` and its encrypted
`utxo_data` is the Borsh-encoded terms. The terms commit the maker's owner (signing key and
nullifier pubkey) rather than `maker_address`, so the taker's registry resolution is checked against
that owner; the viewing key only encrypts the maker payout. See the README.

`taker_pk_fe` selects who may take. `0` makes an open order: any holder of the order UTXO hash
preimage with a matching destination UTXO may take it. A nonzero value designates the taker: it is
`solana_owner_identity` of the taker's Solana pubkey, the same owner pk field encoding as the cancel circuit's
`maker_owner_pk_field`, and only the taker whose destination-side input UTXO owner hash equals
`Poseidon(taker_pk_fe, taker_nullifier_pk)` may take. The take proof witnesses `taker_nullifier_pk`
privately. The value is committed like every other term, so neither party can change it after make.

`source_asset_id`, `source_amount`, and `owner` are the order UTXO's own SPP fields, already
committed in `utxo_hash`. The order UTXO's owner is the maker's order-authority PDA (seeds
`[b"order_authority", maker]`) and its nullifier secret is hardcoded to 0, so:

```text
order_authority_pda   = find_program_address([b"order_authority", maker], program_id)
order_utxo_owner_hash = Poseidon(solana_owner_identity(order_authority_pda), Poseidon(0))   // one per maker
nullifier             = Poseidon(utxo_hash, blinding, 0)                                    // recomputed from the preimage
```

The make proof binds the order output's owner to this hash, which the program computes from the
`maker` signer (see the [make circuit](#make-circuit)). SPP alone would accept any owner that
signs the transact, including the maker, who signs for the source; the proof check is what keeps a
maker from owning the order UTXO itself, or putting it under another maker's PDA, and spending it
before expiry without the program.

Knowledge of the order UTXO hash preimage, the order terms plus the order UTXO `blinding`, is the complete
spend capability: the nullifier includes the `blinding` and the circuits need it to recompute the
order UTXO `utxo_hash`. The preimage is delivered in the make
transaction (see
[make](#make)): the order UTXO output's recipient ciphertext contains the order terms other
than `maker_address`, encrypted to the taker's viewing pubkey (the gnark flow; Xark carries the
Borsh-encoded terms, the maker's owner included, as the output's `utxo_data`, see the Xark note
above), and the maker can decrypt the same slot via the transaction viewing key it holds. The taker resolves `maker_address`
from the user registry via the marker's `maker_pubkey`, the `make`
signer, and verifies the resolution by recomputing the order UTXO `utxo_hash`. A maker finds its
own open orders by deriving its order-authority PDA and fetching the transactions tagged with it.
Moving the order UTXO requires the program: SPP spends a PDA-owned UTXO only when the swap program
produces that PDA's signature via `invoke_signed`, which it does only through `take` (constrained
to the committed payout) or `cancel` (after expiry, maker-signed, to `maker_address`). The take
and cancel circuits leave the order UTXO input's owner a free witness; SPP enforces it at spend
time, when the input's owner must match the PDA signer, and its transact proof binds the owner
signers, so a take naming the wrong maker fails SPP verification. The program derives the PDA
via `find_program_address` from the maker (the `maker` signer at make and cancel, the `maker`
instruction-data field at take), checks it is present among the forwarded SPP accounts, and flips
it to a signer inside the SPP CPI with seeds `[b"order_authority", maker, bump]`. The PDA appears in the ordered owner
signer section after the fixed `[payer, output_tree, spp_program, system_program]`
prefix and the input-tree and nullifier runs; it is a bare address and signs only inside the CPI.

`maker_address` is the committed destination for both outcomes: take pays the destination output
there, cancel returns the source output there. Either way the maker recovers the bought output from
the order UTXO blinding it already holds: `take` derives the payout blinding seed from the order
blinding, and the maker reconstructs the payout using the first published nullifier.
Cancel requires the maker: it signs the cancel transaction, and the cancel
proof checks `solana_owner_identity` of the signer's pubkey against the order's
`maker_owner_hash`. The refund can only land at `maker_address`.

`expiry` is a unix-seconds value the proof reveals as a public input and the swap program checks
against the Clock sysvar: `take` requires `now <= expiry`, `cancel`
requires `now > expiry`. The instructions source the revealed `expiry` differently. `take` reuses the transact's own
`transact.expiry_unix_ts` field as the order expiry, whereas `cancel` takes the order expiry as a
separate `order_expiry` instruction-data field distinct from `transact.expiry_unix_ts` (the SPP
relayer deadline).
Either way the proof's public `expiry` must equal the committed order term, so neither party can
shift the window.

## Instructions

| # | Instruction | Tag | Description | Accounts Read | Accounts Modified | Access control |
|---|-------------|-----|-------------|---------------|-------------------|----------------|
| 1 | [make](#make) | 2 | Verify the make proof and CPI SPP `transact` to lock the source funds into the order UTXO, owned by the maker's order-authority PDA, and write the taker's marker message. | order_authority | SPP trees (CPI) | Maker signs (fee payer); the proof checks the order UTXO owner against the PDA the program derives from the maker |
| 2 | [take](#take) | 3 | Verify the take proof (standard Groth16) and CPI SPP `transact`: spend the order UTXO + a matching destination UTXO, pay destination to the maker (blinding derived from the order UTXO blinding, standard output ciphertext) and source to the taker. | order_authority | SPP trees (CPI) | Any fee payer signs; the order UTXO spend is authorized by the order-authority PDA of the `maker` the instruction names; the maker payout is constrained to the committed `maker_address` |
| 3 | [cancel](#cancel) | 4 | Verify the cancel proof and CPI SPP `transact`: after expiry, spend the order UTXO back to `maker_address`. | order_authority | SPP trees (CPI) | Maker signs; the proof checks the signer against the committed `maker_owner_hash`; the maker's order-authority PDA authorizes the order UTXO spend |

---

### make

Opens an order. The swap program verifies the [make proof](#make-circuit), then CPIs SPP
[`transact`](../../docs/spec.md#transact) to spend the maker's `source_asset_id` UTXO and append the
order UTXO, a UTXO of `source_amount` `source_asset_id` owned by the maker's order-authority PDA
(seeds `[b"order_authority", maker]`), with the [order terms](#order-terms) in its `utxo_data`.
The program derives the PDA from the `maker` signer, computes the order owner the make proof must
match, and signs the SPP CPI as that PDA. The transact is 1-in/2-out
(the maker's source UTXO in; a change UTXO to the maker and the order UTXO out) plus a marker
message tagged to the taker.

The order UTXO output's recipient ciphertext (`TransferRecipientPlaintext { asset_id, amount, blinding,
ring_program_id, data }`, with `data` = the order terms other than `maker_address`; the Xark
implementation's `data` is the Borsh-encoded terms, the maker's owner included) is encrypted to
the taker's viewing pubkey, so the taker recovers the private order terms and the order UTXO `blinding`;
the maker can decrypt the same slot via the transaction viewing key it holds. Those two parties are
exactly who can decrypt the order. The taker resolves `maker_address`
from the user registry via the marker's `maker_pubkey` and verifies the resolution by
recomputing the order UTXO `utxo_hash`. The program requires exactly one transact message, with
`data` equal to the 64-byte zeroed placeholder (`MARKER_PLACEHOLDER`), and overwrites it in place
with the plaintext [`MarkerData`](#glossary) `{ order_utxo_hash,
maker_pubkey }` (the order UTXO hash read from transact output index 1, the pubkey from the
signer), so ordinary wallet sync finds the trade and
can locate the matching order UTXO slot to decrypt. The message is committed by the SPP
proof through the external-data hash, so the SPP proof only verifies for the exact
`MarkerData` the program writes; only the marker's `view_tag` is unenforced: a wrong tag means the
taker does not index the trade.

Both implementations bind the committed maker to the `maker` signer through a public
`maker_owner_pk_field`, `solana_owner_identity` of the signer, so the marker's `maker_pubkey`, the
PDA that owns the order and the maker the terms commit are the same. In both implementations the
source UTXO is owned by the maker, who signs the transact as its owner signer, ahead of its
order-authority PDA, which signs for the data-bearing order output; the change returns to the
maker. Xark: the make circuit takes the source owner as the order's maker. gnark: the circuit
checks the terms' `maker_owner_hash` is `Poseidon(maker_owner_pk_field, maker_nullifier_pk)` and
that the source's owner hash equals it. A UTXO of the maker's order-authority PDA, such as an open
order, therefore cannot fund a make, so a maker cannot pull an order before expiry through make.

The proof checks the order UTXO output against the order rules (see the [make
circuit](#make-circuit)) without revealing the terms, and commits the transaction via
`private_tx_hash`, with `maker_owner_pk_field` and the order owner the program computes. The source asset, the
amounts, and the other order terms are private at the swap layer (the transact's own `asset_id`
public inputs still reveal `source_asset_id` at the SPP layer).

**Accounts**

1. `maker` — owns and spends the source UTXO; signer, writable (fee payer). Consumed by the
   program (its pubkey enters the marker and derives the order-authority PDA); everything after it
   is forwarded verbatim to the SPP `transact` CPI.
2. The SPP `transact` accounts, in SPP's order: the fixed `[payer, output_tree, spp_program,
   system_program]` prefix (the payer is the maker again), the input-tree and nullifier runs, then
   the ordered owner signers: the `maker` again, as the source's owner signer, and last the
   maker's `order_authority` PDA, read-only and non-signer, which the program flips to a signer
   inside the CPI.

**Instruction data**

```rust
struct MakeIxData {
    /// The make proof; verified by the swap program.
    proof: MakeProof,
    /// Xark only: the viewing pubkey of the order UTXO's owner address (the taker's viewing
    /// key the order is encrypted to), which with the PDA's owner hash forms the order-owner
    /// public input. It decides who can decrypt the order, never who can spend it.
    viewing_pubkey: [u8; 33],
    /// SPP transact (1-in/2-out): maker source UTXO -> change + order UTXO;
    /// includes the marker message.
    transact: TransactIxData,
}
```

---

### take

A taker settles the order before expiry. The swap program verifies the
[take proof](#take-circuit) (standard Groth16), then CPIs SPP
[`transact`](../../docs/spec.md#transact). The transact is 2-in/2-out: the order UTXO and a
`destination_amount` `destination_asset_id` UTXO in; `source_amount` `source_asset_id` to that
input's owner and `destination_amount` `destination_asset_id` to `maker_address` out. The swap
conserves value per asset; the taker's profit is the spread already priced into
`destination_amount`. The proof fixes the destination blinding to SPP's output blinding derivation
under `Poseidon("SWTX", order_utxo_blinding)` and the first published nullifier, so the maker
reconstructs and spends the bought output from the order UTXO blinding and order terms alone; that
output carries an ordinary recipient ciphertext for wallet discovery. The circuit checks the payout,
the blinding derivation and the committed `taker_pk_fe`: on an open order (`taker_pk_fe = 0`) any
holder of the order UTXO hash preimage (the maker and the taker) and a matching destination UTXO may
take, while on a designated order the destination-side input must belong to the designated taker.
Either way the maker payout flows only to the committed `maker_address`. The swap program supplies the order-authority PDA signer of the `maker` the instruction names (the taker reads it from the marker's `maker_pubkey`) for the order UTXO spend,
reads the order `expiry` from the transact's own `expiry_unix_ts` field and checks it against the
Clock sysvar (`now <= expiry`); the take proof takes that same value as a public input.

**Accounts**

1. `taker` — fee payer; signer, writable. Consumed by the program; everything after it is forwarded
   verbatim to the SPP `transact` CPI. `now` is read from the Clock sysvar via syscall.
2. The SPP `transact` accounts, in SPP's order: the fixed `[payer, output_tree, spp_program,
   system_program]` prefix, the input-tree and nullifier runs, then the ordered owner signers,
   among them the order-authority PDA of the instruction's `maker` (seeds `[b"order_authority",
   maker]`), read-only and non-signer. The program flips it to a signer inside the SPP CPI to
   authorize the order UTXO spend (see [Order Terms](#order-terms)).

**Instruction data**

```rust
struct TakeIxData {
    /// The take proof; verified by the swap program (standard Groth16).
    proof: TakeProof,
    /// The maker's Solana pubkey, from the marker's `maker_pubkey`; derives the order-authority
    /// PDA. A wrong value makes SPP reject the transact.
    maker: [u8; 32],
    /// SPP transact (2-in/2-out): order UTXO + destination UTXO -> source to the taker,
    /// destination to the maker.
    transact: TransactIxData,
}
```

---

### cancel

After expiry, the order UTXO is reclaimed to the committed `maker_address`. The swap program verifies
the [cancel proof](#cancel-circuit), then CPIs SPP [`transact`](../../docs/spec.md#transact). The
transact is 1-in/1-out: the order UTXO in, a `source_amount` `source_asset_id` UTXO to
`maker_address` out, proved at the SPP `(1, 2)` shape with compact padding in the second output
slot, which the instruction data leaves out. The maker signs as a dedicated readonly signer; the program includes
`solana_owner_identity` of its pubkey in the proof's public input and the circuit checks it against the
committed
`maker_owner_hash`, so only the maker can cancel, and the maker knows the refund blinding it chose.
The swap program supplies the `maker` signer's order-authority PDA signer
via `invoke_signed` and reads the order `expiry` from the dedicated `order_expiry`
instruction-data field (distinct from `transact.expiry_unix_ts`, the SPP relayer deadline) and
checks it against the Clock sysvar (`now > expiry`); the cancel proof takes that same value as a
public input.

**Accounts**

1. `caller` — fee payer; signer, writable. Consumed by the program. `now` is read from
   the Clock sysvar via syscall.
2. `maker` — the maker's Solana signer; read-only, signer. Consumed by the program, which includes
   `solana_owner_identity(maker)` in the cancel proof's public input; everything after it is forwarded
   verbatim to the SPP `transact` CPI.
3. The SPP `transact` accounts, in SPP's order: the fixed `[payer, output_tree, spp_program,
   system_program]` prefix, the input-tree and nullifier runs, then the owner signer: the `maker`
   signer's order-authority PDA (seeds `[b"order_authority", maker]`), read-only and non-signer.
   The program flips it to a signer inside the SPP CPI to authorize the order UTXO spend (see
   [Order Terms](#order-terms)).

**Instruction data**

```rust
struct CancelIxData {
    /// The cancel proof; verified by the swap program.
    proof: CancelProof,
    /// The committed order expiry, checked against the Clock (now > expiry) and a proof public
    /// input. Distinct from transact.expiry_unix_ts (the SPP relayer deadline).
    order_expiry: u64,
    /// SPP transact (1-in/1-out): order UTXO -> source UTXO to maker_address.
    transact: TransactIxData,
}
```

## Circuits

The swap program runs three circuits, each with its own verifying key, distinct from the SPP value
proof inside `transact`. Each circuit witnesses the SPP transaction's UTXO hash preimages (the order UTXO
input, the outputs), enforces the order rules below, and commits the transaction via
`private_tx_hash`: each circuit hashes it with its public values into a single public input. SPP proves
the UTXOs are in the tree and conserves value; the swap circuits rely on that rather than proving
membership themselves. All three are standard Groth16. Concrete shape parameters (input/output slot counts) are fixed once and
benchmarked, and must match the SPP `transact` shapes the instructions use. The circuits are small
and proven in-process, through a gnark→Rust FFI binding or through Xark; the SPP transfer proof
still comes from the existing SPP prover. The public-input hashes below are the gnark
implementation's; the Xark implementation folds the same values as
`Poseidon(values..., private_tx_hash)`, and its circuits build the transaction with the program
SDK's UTXO and transaction operations, which derive the same blindings and `private_tx_hash` (see
the [README](README.md#circuits)).

### Make circuit

Proves the order UTXO output commits the order terms. Matches the 1-in/2-out transact
(source UTXO in; change + order UTXO out), padded to the SPP `(2, 2)` proving shape. The marker message
is committed by the SPP proof through the external-data hash, which the make proof does not read.

- **Public inputs:** `Poseidon(private_tx_hash, order_owner_hash, maker_owner_pk_field)`, where
  the program takes `private_tx_hash` from `TransactIxData`, computes `order_owner_hash`, the owner
  hash of the `maker` signer's order-authority PDA, and `maker_owner_pk_field`,
  `solana_owner_identity` of the signer. Xark binds the whole order owner address,
  `order_owner = Poseidon(order_owner_hash, hash_bytes(viewing_pubkey))` with the instruction's
  `viewing_pubkey`, in place of `order_owner_hash`.
- **Private inputs:** the order terms (`destination_asset`, `destination_amount`, the maker's
  owner hash and viewing pubkey, `expiry`, `taker_pk_fe`), the maker's `maker_nullifier_pk`
  (gnark) and the fully witnessed source input UTXO, order UTXO and change output UTXOs.
- **Constraints:**
  - The `private_tx_hash` recomputation mirrors the padded transact exactly: `chain([source_input,
    0])` over inputs and `chain([change, order_utxo])` over outputs, each folding only its nonzero
    entries, and the zero chain over addresses. The source input hash is recomputed from the
    witnessed source UTXO. A zero-amount change output is still a real UTXO whose hash enters the
    output chain.
  - The source input is the maker's: a plain UTXO (default ring, `data_hash = 0`) of the order
    UTXO's asset owned by `maker_owner_hash` (gnark), or whose owner's `owner_pk_field` is the
    public `maker_owner_pk_field` (Xark). SPP additionally requires the source owner to sign.
  - The order UTXO output committed in `private_tx_hash` has `data_hash = Poseidon(order terms)`
    with `maker_address` hashed in as a field element, ring fields 0, and a nonzero amount, so the
    public SPP order UTXO output commits the terms.
  - The change output is constrained to the order UTXO's asset and to `maker_owner_hash`, with empty
    data.
  - `destination_amount > 0` (64-bit range-checked).
  - The order UTXO output's owner is the public order owner, the maker's order-authority PDA, so
    only the program can spend it, and only for that maker.
  - The committed `maker_owner_hash` belongs to the signer: gnark checks
    `Poseidon(maker_owner_pk_field, maker_nullifier_pk) == maker_owner_hash`.
- **Xark:** the source owner's signing key identity equals the public `maker_owner_pk_field`;
  the change is the SDK's implicit change to the source owner, and the order terms' maker is that
  owner.

### Take circuit

Standard Groth16. Pays the destination output to the maker and checks the payout
against the committed terms, matching the 2-in/2-out transact (order UTXO + destination UTXO in;
source + destination-to-maker out). The program enforces `now <= expiry` against the Clock;
the circuit only reveals `expiry` and checks it equals the committed term.

- **Public inputs:** `Poseidon(private_tx_hash, expiry, first_nullifier)`. The program reads
  `first_nullifier` from the first SPP input, so it cannot be substituted by the prover.
- **Private inputs:** the order UTXO hash preimage (order terms, `source_amount`,
  `order_utxo_blinding`), the maker's compressed viewing pubkey (feeding `maker_address`), the taker's
  destination-side input UTXO, the taker's `taker_nullifier_pk`, and the two output UTXO hash
  preimages.
- **Constraints:**
  - The public `expiry` equals the committed order `expiry`.
  - If the committed `taker_pk_fe` is nonzero, the destination-side input's owner hash equals
    `Poseidon(taker_pk_fe, taker_nullifier_pk)`; if it is `0`, the input owner is unconstrained. One
    constraint expresses both cases: `taker_pk_fe * (owner_hash - Poseidon(taker_pk_fe,
    taker_nullifier_pk)) = 0`.
  - The destination-side input is `(destination_asset_id, destination_amount)`; the outputs
    committed in `private_tx_hash` are `destination_output == (destination_asset_id,
    destination_amount, maker_owner_hash)` and `source_output == (source_asset_id, source_amount)`
    owned by the destination-side input's owner, so the taker pays itself the source funds.
  - `blinding_seed = Poseidon("SWTX", order_utxo_blinding)`, where `SWTX` is the ASCII
    tag as a 32-bit integer. Both the private transaction blinding and both output
    blindings must follow SPP's blinding seed derivations under this seed and
    the first published nullifier. Output slots are source `0`, maker destination `1`.
    The maker reconstructs its payout from the order opening and that nullifier,
    even if the taker omits or corrupts every settlement ciphertext.
  - `destination_amount > 0` (64-bit range-checked); the order UTXO amount is nonzero.
- **Xark:** the payout owner is the committed maker owner; a private `maker_address` input
  only names the viewing key the payout ciphertext is encrypted to.

### Cancel circuit

Reclaims the order UTXO to the committed `maker_address` after expiry. Matches the 1-in/1-out
transact (order UTXO in; source-to-maker out), proved at the SPP `(1, 2)` shape with a compact
padding second output that the private transaction hash does not see. The program enforces `now > expiry` against the Clock; the
circuit only reveals `expiry` and checks it equals the committed term.

- **Public inputs:** `Poseidon(private_tx_hash, expiry, maker_owner_pk_field)`, where
  `maker_owner_pk_field` is `solana_owner_identity` of the maker signer's pubkey, fed by the program.
- **Private inputs:** the order UTXO hash preimage (incl. `utxo_data` = order terms,
  `source_amount`, `order_utxo_blinding`), the `source_output` hash preimage, and the maker's
  `(maker_owner_pk_field,
  maker_nullifier_pk)`, the preimage of the committed `maker_owner_hash`. The order UTXO owner is a
  free witness: SPP spends it only with the PDA signature, which the program produces only for the
  `maker` signer's PDA.
- **Constraints:**
  - The public `expiry` equals the committed order `expiry`.
  - `Poseidon(maker_owner_pk_field, maker_nullifier_pk)` equals the committed `maker_owner_hash`,
    so the proof verifies only with the maker signer the program supplies.
  - The output committed in `private_tx_hash` is `source_output == (source_asset_id, source_amount,
    maker_owner_hash)`.
</content>
</invoke>
