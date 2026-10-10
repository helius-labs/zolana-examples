//! The user's side of a swap: input selection, proving the transfer for an
//! offer, and the checks on the market maker's swap message before the user
//! signs it. Nothing the market maker returns is signed without
//! `QuoteCheck::verify`.

use std::{
    cmp::Reverse,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::Result;
use solana_address::Address;
use solana_instruction::Instruction;
use solana_message::VersionedMessage;
use zolana_client::{ProofAuthority, Rpc, ZolanaClient};
use zolana_transaction::{ShieldedKeys, Utxo, WalletUtxo};

use zolana_interface::{instruction::TransactIxData, pda};

use crate::{
    address::{order_address, ORDER_ADDRESS_TREE},
    message::{instructions, transact_data},
    pair::Pair,
    swap::{Offer, Order, SwapError, SwapRequest},
    transfer::{Receiver, Transfer},
};

/// Index of the first input tree account of a `transact` instruction, after
/// the payer, the output tree, the shielded pool program and the system
/// program (zolana's `Transact::instruction` and SPP's
/// `TransactAccounts::validate_and_parse`). Input tree `tree_index` is the
/// account at `TRANSACT_INPUT_TREES_OFFSET + tree_index`.
const TRANSACT_INPUT_TREES_OFFSET: usize = 4;

/// Picks the user's UTXOs of `asset` to pay `amount`, largest first, so the
/// transfer spends as few inputs as possible.
///
/// Errors with `SwapError::InsufficientFunds` when all UTXOs of `asset` sum
/// to less than `amount`, and with `SwapError::TooManyInputs` when covering
/// it takes more than `max_inputs` of them.
pub fn select_inputs(
    utxos: Vec<WalletUtxo>,
    asset: Address,
    amount: u64,
    max_inputs: usize,
) -> Result<Vec<WalletUtxo>, SwapError> {
    let mut candidates: Vec<WalletUtxo> = utxos
        .into_iter()
        .filter(|utxo| utxo.utxo.asset.asset == asset)
        .collect();
    candidates.sort_by_key(|utxo| Reverse(utxo.utxo.amount));
    let mut inputs = Vec::new();
    let mut total = 0u64;
    for utxo in candidates {
        if total >= amount {
            break;
        }
        total = total.saturating_add(utxo.utxo.amount);
        inputs.push(utxo);
    }
    if total < amount {
        return Err(SwapError::InsufficientFunds {
            asset,
            required: amount,
        });
    }
    if inputs.len() > max_inputs {
        return Err(SwapError::TooManyInputs {
            needed: inputs.len(),
            max: max_inputs,
        });
    }
    Ok(inputs)
}

/// An accepted offer with the inputs that will pay it, ready to prove.
pub struct UserOrder {
    pub offer: Offer,
    /// The UTXOs the transfer spends, from [`select_inputs`].
    pub inputs: Vec<WalletUtxo>,
    /// Proof shape input count to pad to; `None` uses `inputs.len()`.
    pub width: Option<usize>,
    /// The state tree the inputs live in and the outputs go to.
    pub tree: Address,
    /// The zolana id of `tree`, part of each output's commitment.
    pub tree_id: u16,
}

impl UserOrder {
    /// Proves the user's transfer of `quote.amount_in` to the market maker and
    /// wraps it in a fill request for `offer.id`.
    ///
    /// Checks, in order, before proving (a proof is expensive, and the
    /// market maker would reject the fill anyway):
    /// 1. the current unix time in whole seconds is before
    ///    `offer.expires_at`, else `SwapError::OrderExpired`. The market maker
    ///    floors the order's expiry to whole seconds when it fills in
    ///    `expires_at`, so the user may treat an order as expired up to one
    ///    second before the market maker does, never later;
    /// 2. `inputs` fit `offer.max_user_inputs`, else
    ///    `SwapError::TooManyInputs`.
    ///
    /// Proving fails with `SwapError::NoSupportedShape` when no proof shape
    /// fits the inputs, or with the prover's error.
    pub fn prove<R: Rpc>(
        self,
        client: &ZolanaClient<R>,
        keys: &dyn ShieldedKeys,
        authority: &dyn ProofAuthority,
    ) -> Result<Order> {
        let UserOrder {
            offer,
            inputs,
            width,
            tree,
            tree_id,
        } = self;
        if SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() >= offer.expires_at {
            return Err(SwapError::OrderExpired { order: offer.id }.into());
        }
        if inputs.len() > offer.max_user_inputs {
            return Err(SwapError::TooManyInputs {
                needed: inputs.len(),
                max: offer.max_user_inputs,
            }
            .into());
        }
        let quote = offer.quote;
        let width = width.unwrap_or(inputs.len());
        let transfer = Transfer {
            inputs,
            width,
            amount: quote.amount_in,
            recipient: offer.market_maker,
            payer: offer.fee_payer,
            tree,
            tree_id,
        }
        .prove(client, keys, authority)?;
        Ok(Order {
            offer,
            request: SwapRequest {
                order: offer.id,
                user: keys.address()?,
                transfer: transfer.instruction,
            },
        })
    }
}

/// The user's pre-signing check of a swap message against its order.
pub struct QuoteCheck<'a> {
    pub order: &'a Order,
    pub message: &'a VersionedMessage,
    pub pair: &'a Pair,
}

impl QuoteCheck<'_> {
    /// Checks the swap message the market maker returns for `order` before the
    /// user signs it. Checks, in order:
    ///
    /// 1. the first account key (the fee payer) is `offer.fee_payer`, else
    ///    `SwapError::UnexpectedTransaction`;
    /// 2. every instruction decodes against the static account keys and
    ///    there are exactly two, else `SwapError::UnexpectedTransaction`;
    /// 3. the first instruction equals `request.transfer` in program id,
    ///    accounts (with their signer and writable flags) and data, else
    ///    `SwapError::UserTransferAltered`;
    /// 4. the market maker's transfer (the second instruction) names none of
    ///    the user's signing keys (the signers of the user's transfer other
    ///    than `offer.fee_payer`) as a signer, else
    ///    `SwapError::UnexpectedSigner`;
    /// 5. the signers of the market maker's transfer are exactly
    ///    `{offer.fee_payer}`: any other signer fails with
    ///    `SwapError::UnexpectedSigner`, a missing fee payer with
    ///    `SwapError::UnexpectedTransaction`. The order address slot adds no
    ///    signer: its owner is the fee payer;
    /// 6. both instructions are zolana `transact`s, else
    ///    `SwapError::UnexpectedTransaction`;
    /// 7. neither transfer carries an interface transfer, else
    ///    `SwapError::PublicTransfer`;
    /// 8. the market maker's transfer spends the order address
    ///    `order_address(offer.fee_payer, offer.id)`: one of its inputs has
    ///    that nullifier and a `tree_index` whose tree account in the
    ///    instruction is `pda::tree(ORDER_ADDRESS_TREE)`, else
    ///    `SwapError::OrderAddressMissing`;
    /// 9. every output of the market maker's transfer the user can decrypt
    ///    opens to its commitment, else `SwapError::CommitmentMismatch`;
    /// 10. exactly one of those outputs is of the quote's output asset, else
    ///     `SwapError::UnexpectedOutputs`;
    /// 11. that output holds at least `offer.quote.amount_out`, else
    ///     `SwapError::BelowQuote`.
    ///
    /// The exact two-instruction shape and the signer checks are what keep
    /// the user's signature from authorizing anything but the user's own
    /// transfer: the user signs the whole message, so a key of the user's
    /// listed in another instruction would be signed for there too. The
    /// order address makes a second fill of the same order fail on-chain:
    /// SPP rejects a nullifier whose PDA exists with `NullifierAlreadyQueued`
    /// (custom code 7043), and a nullifier already in the tree fails the
    /// slot's non-inclusion proof.
    pub fn verify(&self, receiver: &Receiver) -> Result<()> {
        let order = self.order;
        if self.message.static_account_keys().first() != Some(&order.offer.fee_payer) {
            return Err(SwapError::UnexpectedTransaction.into());
        }
        let instructions = instructions(self.message)?;
        let [user_transfer, market_maker_transfer] = instructions.as_slice() else {
            return Err(SwapError::UnexpectedTransaction.into());
        };
        if *user_transfer != order.request.transfer {
            return Err(SwapError::UserTransferAltered.into());
        }
        let offer = &order.offer;
        check_signers(&offer.fee_payer, user_transfer, market_maker_transfer)?;
        let user_transact = transact_data(user_transfer)?;
        let market_maker_transact = transact_data(market_maker_transfer)?;
        let count = user_transact
            .interface_transfers
            .len()
            .checked_add(market_maker_transact.interface_transfers.len())
            .ok_or(SwapError::AmountOverflow {
                context: "interface transfer count",
            })?;
        if count != 0 {
            return Err(SwapError::PublicTransfer { count }.into());
        }
        check_order_address(offer, market_maker_transfer, &market_maker_transact)?;
        let (_, asset_out) = order.offer.quote.direction.assets(self.pair);
        let received: Vec<Utxo> = receiver
            .received_outputs(&market_maker_transact)?
            .into_iter()
            .map(|(utxo, _)| utxo)
            .filter(|utxo| utxo.asset.asset == asset_out)
            .collect();
        let [utxo] = received.as_slice() else {
            return Err(SwapError::UnexpectedOutputs {
                received: received.len(),
            }
            .into());
        };
        let quoted = order.offer.quote.amount_out;
        if utxo.amount < quoted {
            return Err(SwapError::BelowQuote {
                quoted,
                offered: utxo.amount,
            }
            .into());
        }
        Ok(())
    }
}

/// Step 8 of [`QuoteCheck::verify`]: some input of `transact`, the decoded
/// data of `instruction`, publishes the order address of `offer` and names
/// the order address tree. The tree is resolved through the instruction's
/// accounts: input tree `tree_index` is account
/// `TRANSACT_INPUT_TREES_OFFSET + tree_index`, which SPP checks is the tree
/// the input's nullifier is inserted into. Errors with
/// `SwapError::OrderAddressMissing`, or `SwapError::OrderAddressDerivation`
/// if the address does not derive.
fn check_order_address(
    offer: &Offer,
    instruction: &Instruction,
    transact: &TransactIxData,
) -> Result<(), SwapError> {
    let address = order_address(&offer.fee_payer, offer.id)?;
    let address_tree = pda::tree(ORDER_ADDRESS_TREE);
    let carried = transact.inputs.iter().any(|input| {
        input.nullifier_hash == address
            && instruction
                .accounts
                .iter()
                .skip(TRANSACT_INPUT_TREES_OFFSET)
                .nth(usize::from(input.tree_index))
                .is_some_and(|account| account.pubkey == address_tree)
    });
    carried
        .then_some(())
        .ok_or(SwapError::OrderAddressMissing { order: offer.id })
}

/// The signer keys `instruction` names, in account order.
fn signers(instruction: &Instruction) -> impl Iterator<Item = &Address> {
    instruction
        .accounts
        .iter()
        .filter(|meta| meta.is_signer)
        .map(|meta| &meta.pubkey)
}

/// Steps 4 and 5 of [`QuoteCheck::verify`]. A decompiled instruction marks an
/// account as a signer whenever the message requires its signature, so a user
/// key listed anywhere in the market maker's transfer shows up here as a
/// signer.
fn check_signers(
    fee_payer: &Address,
    user_transfer: &Instruction,
    market_maker_transfer: &Instruction,
) -> Result<(), SwapError> {
    let user_keys: Vec<&Address> = signers(user_transfer)
        .filter(|signer| *signer != fee_payer)
        .collect();
    if let Some(signer) = signers(market_maker_transfer).find(|signer| user_keys.contains(signer)) {
        return Err(SwapError::UnexpectedSigner { signer: *signer });
    }
    if let Some(signer) = signers(market_maker_transfer).find(|signer| *signer != fee_payer) {
        return Err(SwapError::UnexpectedSigner { signer: *signer });
    }
    if !signers(market_maker_transfer).any(|signer| signer == fee_payer) {
        return Err(SwapError::UnexpectedTransaction);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use solana_instruction::AccountMeta;
    use solana_message::{legacy::Message, VersionedMessage};
    use zolana_interface::PROGRAM_ID_PUBKEY;
    use zolana_keypair::ShieldedKeypair;
    use zolana_transaction::AssetRegistry;

    use super::*;
    use crate::swap::{Direction, OrderId, Quote};

    /// A zolana-addressed instruction naming `accounts`; its data is never
    /// decoded because the signer checks run first.
    fn transact(accounts: Vec<AccountMeta>) -> Instruction {
        Instruction {
            program_id: PROGRAM_ID_PUBKEY,
            accounts,
            data: vec![0],
        }
    }

    /// `QuoteCheck::verify` on `[user_transfer, market_maker_transfer]` paid
    /// by `fee_payer`, with the error it returns as a `SwapError`.
    fn verify_error(
        fee_payer: Address,
        user_transfer: Instruction,
        market_maker_transfer: Instruction,
    ) -> Option<SwapError> {
        let keypair = ShieldedKeypair::new_p256().ok()?;
        let address = keypair.shielded_address().ok()?;
        let id = OrderId([7; 16]);
        let message = VersionedMessage::Legacy(Message::new(
            &[user_transfer.clone(), market_maker_transfer],
            Some(&fee_payer),
        ));
        let offer = Offer {
            id,
            expires_at: u64::MAX,
            quote: Quote {
                direction: Direction::Deposit,
                amount_in: 100,
                amount_out: 100,
            },
            market_maker: address,
            fee_payer,
            max_user_inputs: 1,
            user_outputs: 2,
        };
        let order = Order {
            offer,
            request: SwapRequest {
                order: id,
                user: address,
                transfer: user_transfer,
            },
        };
        let pair = Pair::new(Address::new_unique(), Address::new_unique());
        let registry = AssetRegistry::default();
        let receiver = Receiver {
            keys: &keypair,
            registry: &registry,
            tree_id: 0,
        };
        QuoteCheck {
            order: &order,
            message: &message,
            pair: &pair,
        }
        .verify(&receiver)
        .err()?
        .downcast_ref::<SwapError>()
        .cloned()
    }

    /// A market maker transfer that names the user's signing key, or any signer
    /// but the fee payer, fails with `UnexpectedSigner` before anything is
    /// decoded.
    #[test]
    fn verify_rejects_signers_outside_the_user_transfer() {
        let fee_payer = Address::new_unique();
        let user_key = Address::new_unique();
        let stranger = Address::new_unique();
        let user_transfer = transact(vec![
            AccountMeta::new(fee_payer, true),
            AccountMeta::new_readonly(user_key, true),
        ]);
        for (label, market_maker_accounts, want) in [
            (
                "market maker transfer names the user's key",
                vec![
                    AccountMeta::new(fee_payer, true),
                    AccountMeta::new_readonly(user_key, true),
                ],
                SwapError::UnexpectedSigner { signer: user_key },
            ),
            (
                "market maker transfer names another signer",
                vec![
                    AccountMeta::new(fee_payer, true),
                    AccountMeta::new_readonly(stranger, true),
                ],
                SwapError::UnexpectedSigner { signer: stranger },
            ),
            (
                "market maker transfer without the fee payer as signer",
                vec![AccountMeta::new_readonly(stranger, false)],
                SwapError::UnexpectedTransaction,
            ),
        ] {
            let got = verify_error(
                fee_payer,
                user_transfer.clone(),
                transact(market_maker_accounts),
            );
            assert_eq!(
                got,
                Some(want.clone()),
                "{label}: got {got:?}, want {want:?}"
            );
        }
    }
}
