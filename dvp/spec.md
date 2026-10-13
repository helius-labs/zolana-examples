# DvP

Delivery-versus-payment (DvP) settles a trade between two parties atomically: the seller's delivery leg and the buyer's payment leg move in the same transaction, or neither moves. Each party escrows its leg ahead of time, and a designated settlement authority settles both legs once the trade is funded. Until settlement, either party can abort and recover its funds.

## Glossary

| Term | Type | Definition |
|---|---|---|
| Terms | Agreement | The `SwapDvp` fields both parties agree to: parties, mints, amounts, settlement authority, time bounds, and settlement destinations. See [Data UTXOs](#data-utxos). |
| DvP ID | Hash | `hash(SwapDvp)`. Identifies the DvP and seeds its owner PDA. |
| DvP UTXO | Data UTXO | A data UTXO whose `data_hash` is the DvP ID, so it commits to the terms (`SwapDvp`). Owned by the DvP PDA. A party accepts the terms by funding its leg. |
| DvP PDA | PDA | `[b"dvp", dvp_id]` under the DvP program. Owns the DvP's UTXOs, so only the DvP program can spend them. |
| DvP | Trade | The trade as a whole, from create to settle or abort. |
| Seller | Wallet (client) | Funds the delivery leg. Can reject the DvP or reclaim its leg. |
| Buyer | Wallet (client) | Funds the payment leg. Can reject the DvP or reclaim its leg. |
| Settlement authority | Server (venue, broker, or custodian backend) | Settles or cancels the DvP. Can also match the trade and create the DvP UTXO. |
| DvP program | Solana program | Owns the DvP's UTXOs through the DvP PDA. Checks the settlement, abort, and expiry rules when those UTXOs are spent. |
| Token program | Solana program | Moves tokens. |

## Settlement flow

1. **Agree.** The seller and buyer agree on the terms (`SwapDvp`).
2. **Create.** The creator builds the DvP UTXO: a data UTXO owned by the DvP PDA that commits to the terms (`SwapDvp`). The same transaction publishes the terms encrypted to the seller, buyer, and settlement authority as three messages. No funds move.
3. **Fund.** Each party decrypts its message and checks the terms against `dvp_id`, then deposits its leg into its escrow. The seller funds the delivery leg and the buyer funds the payment leg.
4. **Settle.** Once both legs are funded and before expiry, the settlement authority settles. In one transaction the delivery leg goes to the buyer, the payment leg goes to the seller, and the DvP closes.
5. **Abort.** Before settlement, either party can reclaim its own leg and leave the DvP open, or reject it. The settlement authority can cancel it. Reject and cancel refund every funded leg to its depositor and close the DvP.

### Properties

## Constants

## Data UTXOs

`SwapDvp` is not stored in an account. Its hash is the DvP UTXO's `data_hash`, and its plaintext is published encrypted in transaction messages. It keeps the field types and order of the reference program (`program/src/state/swap_dvp.rs`), except `bump`, which is removed: the PDA is derived from `dvp_id`, so its bump cannot be part of the hashed data. `blinding` is added at the end. Fields are renamed: `user_a` -> `seller`, `user_b` -> `buyer`, `*_a` -> `delivery_*`, `*_b` -> `payment_*`.

- `dvp_id = hash(SwapDvp)`, computed over the plaintext terms.
- The DvP UTXO's `data_hash` is `dvp_id`, and its `utxo_data` is empty.
- The DvP UTXOs are owned by the PDA with seeds `[b"dvp", dvp_id]`.
- The creator knows the viewing public keys of the seller, buyer, and settlement authority. It encrypts the plaintext `SwapDvp` to each of them and publishes the three ciphertexts as messages in the create `transact` (`TransactIxData::messages`, one `MessageData { view_tag, data }` per recipient). Encryption follows the zolana AES key derivation; message slots continue the ciphertext numbering after the outputs.
- A recipient decrypts its message, recomputes `hash(SwapDvp)`, and checks it against `dvp_id`.

```rust
pub struct SwapDvp {
    pub seller: Pubkey,
    pub buyer: Pubkey,
    pub delivery_mint: Pubkey,
    pub payment_mint: Pubkey,
    pub settlement_authority: Pubkey,
    /// Owner of `delivery_mint`, read from the mint at create.
    pub delivery_token_program: Pubkey,
    /// Owner of `payment_mint`, read from the mint at create.
    pub payment_token_program: Pubkey,
    pub delivery_amount: u64,
    pub payment_amount: u64,
    /// Settlement fails after this time. Cluster time (`Clock::unix_timestamp`).
    pub expiry_timestamp: i64,
    /// Distinguishes DvPs with otherwise identical terms.
    pub nonce: u64,
    /// Client reference, UTF-8 zero-padded. The DvP program does not read it.
    pub ref_string: [u8; 64],
    /// Receives the payment leg at settlement. Defaults to `seller`.
    pub seller_settlement_destination: Pubkey,
    /// Receives the delivery leg at settlement. Defaults to `buyer`.
    pub buyer_settlement_destination: Pubkey,
    /// Mint authority of `delivery_mint` at create, or all zeros if none.
    /// Settlement fails if it changed.
    pub delivery_mint_authority: Pubkey,
    /// Mint authority of `payment_mint` at create. Same rule.
    pub payment_mint_authority: Pubkey,
    /// Settlement fails before this time, if set.
    pub earliest_settlement_timestamp: Option<i64>,
    /// Random field element. Without it, guessed terms can be checked against `dvp_id`.
    pub blinding: [u8; 32],
}
```

## Accounts

## Instructions

## Limitations
