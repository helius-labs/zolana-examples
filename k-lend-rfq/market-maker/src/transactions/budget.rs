//! Transaction sizing. Every limit the market maker advertises or enforces
//! (user transfer width, market maker inputs and outputs, consolidation width)
//! is found by building placeholder instructions of the real size and measuring
//! the compiled v1 transaction, so the limits track the zolana encoding
//! exactly.

use solana_address::Address;
use solana_instruction::Instruction;
use thiserror::Error;
use zolana_client::{transaction_size, ClientError, Shape, TransactionSize, SPP_SUPPORTED_SHAPES};
use zolana_interface::{
    instruction::{
        CircuitId, InputUtxo, InterfaceTransfer, OwnerTag, TransactIxData, TransactOutput,
        TransactProof, TreeContext,
    },
    pda, N_PUBLIC_SLOTS,
};
use zolana_keypair::ViewingKey;
use zolana_program::instruction::{
    Transact, TransactInterfaceTransferAccounts, TransactSplWithdrawalAccounts,
};
use zolana_transaction::{
    serialization::{
        confidential::{Confidential, ConfidentialEncode, ConfidentialOutputPlaintext},
        UtxoSerialization,
    },
    Data, TransactionError, SOL_ASSET_ID,
};

use k_lend_rfq_sdk::{
    address::{ORDER_ADDRESS_SLOTS, ORDER_ADDRESS_TREE},
    swap::SwapError,
    transfer::{smallest_shape, USER_OUTPUTS},
};

use crate::swap::fill::SWAP_COMPUTE_BUDGET;

/// Fewest outputs of the market maker's swap transfer: the payment to the user
/// and the market maker's change.
pub const MARKET_MAKER_MIN_OUTPUTS: usize = 2;
/// Solana's per-transaction compute unit limit: a transaction cannot request
/// more than 1.4M units (`MAX_COMPUTE_UNIT_LIMIT`, agave `compute-budget`).
pub const MAX_COMPUTE_UNITS: u32 = 1_400_000;
// The placeholder addresses below only need to be distinct from each other
// and from the market maker's keys, so each account costs its own 32 bytes.
const PLACEHOLDER_USER: Address = Address::new_from_array([7; 32]);
/// Stands in for a user input tree distinct from the market maker's, so the
/// user placeholder pays for a second tree account and tree context.
const PLACEHOLDER_USER_TREE: Address = Address::new_from_array([10; 32]);
const PLACEHOLDER_MINT: Address = Address::new_from_array([8; 32]);
const PLACEHOLDER_TOKEN_ACCOUNT: Address = Address::new_from_array([9; 32]);

/// Why a transaction or a transfer width does not fit.
#[derive(Debug, Error)]
pub enum BudgetError {
    /// No proof shape with at least `inputs` inputs and `outputs` outputs
    /// fits the transaction.
    #[error("no supported shape takes {inputs} inputs and {outputs} outputs")]
    NoSupportedShape { inputs: usize, outputs: usize },
    /// The compiled message exceeds the v1 byte or account address limit.
    #[error("transaction of {bytes} bytes and {addresses} addresses does not fit transaction v1")]
    TransactionTooLarge { bytes: usize, addresses: usize },
    /// `SWAP_COMPUTE_BUDGET` asks for more than `MAX_COMPUTE_UNITS`; checked
    /// once at start.
    #[error("the swap budget of {units} compute units exceeds the ceiling of {max}")]
    ComputeBudgetExceeded { units: u32, max: u32 },
    #[error(transparent)]
    Client(#[from] ClientError),
    #[error(transparent)]
    Transaction(#[from] TransactionError),
    #[error(transparent)]
    Swap(#[from] SwapError),
}

/// The size-relevant features of a placeholder `transact`.
#[derive(Clone, Copy)]
struct TransferTemplate {
    shape: Shape,
    /// A signer account besides the payer (the user signs its own transfer).
    owner_signer: Option<Address>,
    /// A public SPL withdrawal and its accounts (consolidations carry one).
    withdrawal: Option<TransactSplWithdrawalAccounts>,
    /// A second input tree account and tree context.
    extra_input_tree: Option<Address>,
    /// Fills the nullifier hashes, distinct per template, so two placeholders
    /// in one message do not share bytes the way two real transfers never do.
    seed: u8,
}

/// Sizes the market maker's transactions against the transaction v1 limits.
///
/// A swap transaction holds exactly two instructions: the user's transfer and
/// the market maker's transfer. The market maker's transfer carries the order
/// address slot (`k_lend_rfq_sdk::address`, which also states its cost): it
/// takes `ORDER_ADDRESS_SLOTS` input slots of the shape and one nullifier PDA
/// account and, when the market maker's tree is not the order address tree,
/// that tree's account and tree context. Every swap size computed here
/// (`max_user_inputs`, `max_market_maker_inputs`, `max_market_maker_outputs`)
/// sizes the market maker placeholder with them, so the user transfer width
/// advertised in an offer leaves room for the slot.
pub struct SwapBudget {
    market_maker: Address,
    tree: Address,
    data_len: usize,
    /// Widest consolidation with the outputs passed to `new` and a public
    /// withdrawal; computed once at start.
    pub max_consolidate_inputs: usize,
}

impl SwapBudget {
    /// Sizes against `market_maker` as fee payer and `tree` as the
    /// market maker's tree.
    ///
    /// Errors with `BudgetError::ComputeBudgetExceeded` when the swap
    /// compute budget is above the Solana limit, and with
    /// `BudgetError::NoSupportedShape` when no consolidation with
    /// `consolidate_outputs` outputs fits.
    pub fn new(
        market_maker: Address,
        tree: Address,
        consolidate_outputs: usize,
    ) -> Result<Self, BudgetError> {
        check_compute_units()?;
        let mut budget = Self {
            market_maker,
            tree,
            data_len: output_data_len()?,
            max_consolidate_inputs: 0,
        };
        budget.max_consolidate_inputs = budget.max_consolidate_inputs_with(
            consolidate_outputs,
            placeholder_withdrawal(),
            &[],
        )?;
        Ok(budget)
    }

    /// The most inputs a consolidation with at least `outputs` outputs, the
    /// public `withdrawal` and the `tail` instructions after it can take in
    /// one transaction. Errors with `BudgetError::NoSupportedShape` when
    /// none fits.
    pub fn max_consolidate_inputs_with(
        &self,
        outputs: usize,
        withdrawal: TransactSplWithdrawalAccounts,
        tail: &[Instruction],
    ) -> Result<usize, BudgetError> {
        let mut widest = 0;
        for shape in SPP_SUPPORTED_SHAPES
            .into_iter()
            .filter(|shape| shape.n_outputs() >= outputs)
        {
            if self.consolidate_size(shape, withdrawal, tail)?.fits() {
                widest = widest.max(shape.n_inputs());
            }
        }
        if widest == 0 {
            return Err(BudgetError::NoSupportedShape { inputs: 1, outputs });
        }
        Ok(widest)
    }

    /// The size of a placeholder consolidation of `shape` with `withdrawal`,
    /// followed by `tail`.
    pub fn consolidate_size(
        &self,
        shape: Shape,
        withdrawal: TransactSplWithdrawalAccounts,
        tail: &[Instruction],
    ) -> Result<TransactionSize, BudgetError> {
        let consolidate = self.placeholder(TransferTemplate {
            shape,
            owner_signer: None,
            withdrawal: Some(withdrawal),
            extra_input_tree: None,
            seed: 3,
        });
        let instructions: Vec<Instruction> = std::iter::once(consolidate)
            .chain(tail.iter().cloned())
            .collect();
        self.size(&instructions)
    }

    /// The most inputs a user transfer with `USER_OUTPUTS` outputs can take
    /// next to a market maker transfer of shape `market_maker` (its order
    /// address slot included), or `None` if not even the narrowest fits.
    ///
    /// Assumes the user's inputs come from at most two trees; a wider user
    /// transfer is rejected at fill time with `UserTransferTooWide` or
    /// `TransactionTooLarge`.
    pub fn max_user_inputs(&self, market_maker: Shape) -> Result<Option<usize>, BudgetError> {
        let market_maker = self.placeholder(self.market_maker_template(market_maker));
        let mut user_shapes: Vec<Shape> = SPP_SUPPORTED_SHAPES
            .into_iter()
            .filter(|shape| shape.n_outputs() == USER_OUTPUTS)
            .collect();
        user_shapes.sort_by_key(|shape| std::cmp::Reverse(shape.n_inputs()));
        for shape in user_shapes {
            let user = self.placeholder(TransferTemplate {
                shape,
                owner_signer: Some(PLACEHOLDER_USER),
                withdrawal: None,
                extra_input_tree: Some(PLACEHOLDER_USER_TREE),
                seed: 1,
            });
            if self.swap_size(user, market_maker.clone())?.fits() {
                return Ok(Some(shape.n_inputs()));
            }
        }
        Ok(None)
    }

    /// The narrowest shape a market maker swap transfer can have: one input,
    /// the order address slot and `MARKET_MAKER_MIN_OUTPUTS` outputs.
    pub fn narrowest_market_maker(&self) -> Result<Shape, BudgetError> {
        let inputs = 1 + ORDER_ADDRESS_SLOTS;
        smallest_shape(inputs, MARKET_MAKER_MIN_OUTPUTS).ok_or(BudgetError::NoSupportedShape {
            inputs,
            outputs: MARKET_MAKER_MIN_OUTPUTS,
        })
    }

    /// A placeholder of the narrowest user transfer: one input from the
    /// market maker's tree and `USER_OUTPUTS` outputs.
    pub fn narrowest_user_transfer(&self) -> Result<Instruction, BudgetError> {
        let shape = smallest_shape(1, USER_OUTPUTS).ok_or(BudgetError::NoSupportedShape {
            inputs: 1,
            outputs: USER_OUTPUTS,
        })?;
        Ok(self.placeholder(TransferTemplate {
            shape,
            owner_signer: Some(PLACEHOLDER_USER),
            withdrawal: None,
            extra_input_tree: None,
            seed: 1,
        }))
    }

    /// The most UTXO inputs a market maker transfer with
    /// `MARKET_MAKER_MIN_OUTPUTS` outputs can take next to `user_transfer`:
    /// the widest fitting shape's inputs less the order address slot. Errors
    /// with `BudgetError::NoSupportedShape` when none leaves room for one
    /// UTXO input.
    pub fn max_market_maker_inputs(
        &self,
        user_transfer: &Instruction,
    ) -> Result<usize, BudgetError> {
        let mut widest = 0;
        for shape in SPP_SUPPORTED_SHAPES
            .into_iter()
            .filter(|shape| shape.n_outputs() >= MARKET_MAKER_MIN_OUTPUTS)
        {
            let inputs = shape.n_inputs().saturating_sub(ORDER_ADDRESS_SLOTS);
            if inputs <= widest {
                continue;
            }
            let market_maker = self.placeholder(self.market_maker_template(shape));
            if self.swap_size(user_transfer.clone(), market_maker)?.fits() {
                widest = inputs;
            }
        }
        if widest == 0 {
            return Err(BudgetError::NoSupportedShape {
                inputs: 1,
                outputs: MARKET_MAKER_MIN_OUTPUTS,
            });
        }
        Ok(widest)
    }

    /// The most outputs a market maker transfer of at least `inputs` UTXO
    /// inputs and the order address slot can have next to `user_transfer`.
    /// Errors with `BudgetError::NoSupportedShape` when fewer than
    /// `MARKET_MAKER_MIN_OUTPUTS` fit.
    pub fn max_market_maker_outputs(
        &self,
        user_transfer: &Instruction,
        inputs: usize,
    ) -> Result<usize, BudgetError> {
        let mut fitting = 0;
        let slots = inputs.saturating_add(ORDER_ADDRESS_SLOTS);
        for shape in SPP_SUPPORTED_SHAPES
            .into_iter()
            .filter(|shape| shape.n_inputs() >= slots)
        {
            if shape.n_outputs() <= fitting {
                continue;
            }
            let market_maker = self.placeholder(self.market_maker_template(shape));
            if self.swap_size(user_transfer.clone(), market_maker)?.fits() {
                fitting = shape.n_outputs();
            }
        }
        if fitting < MARKET_MAKER_MIN_OUTPUTS {
            return Err(BudgetError::NoSupportedShape {
                inputs,
                outputs: MARKET_MAKER_MIN_OUTPUTS,
            });
        }
        Ok(fitting)
    }

    /// The size of `transfers` compiled with the market maker as fee payer;
    /// errors with `BudgetError::TransactionTooLarge` when it does not fit.
    pub fn check(&self, transfers: &[Instruction]) -> Result<TransactionSize, BudgetError> {
        let size = self.size(transfers)?;
        if size.fits() {
            Ok(size)
        } else {
            Err(BudgetError::TransactionTooLarge {
                bytes: size.bytes,
                addresses: size.addresses,
            })
        }
    }

    fn swap_size(
        &self,
        user: Instruction,
        market_maker: Instruction,
    ) -> Result<TransactionSize, BudgetError> {
        self.size(&[user, market_maker])
    }

    fn size(&self, transfers: &[Instruction]) -> Result<TransactionSize, BudgetError> {
        Ok(transaction_size(
            &self.market_maker,
            transfers,
            SWAP_COMPUTE_BUDGET,
        )?)
    }

    /// A market maker swap transfer of `shape`. The placeholder publishes a
    /// nullifier, with its PDA account, for every input slot, so the order
    /// address slot is sized like any input; the order address tree is an
    /// extra input tree unless it is the market maker's.
    fn market_maker_template(&self, shape: Shape) -> TransferTemplate {
        let address_tree = pda::tree(ORDER_ADDRESS_TREE);
        TransferTemplate {
            shape,
            owner_signer: None,
            withdrawal: None,
            extra_input_tree: (self.tree != address_tree).then_some(address_tree),
            seed: 2,
        }
    }

    /// A `transact` with the byte size of a real one of `template`: zeroed
    /// proof and hashes, outputs carrying ciphertexts of the real length.
    fn placeholder(&self, template: TransferTemplate) -> Instruction {
        let data_len = self.data_len;
        let inputs = (0..template.shape.n_inputs())
            .map(|input_index| {
                let mut nullifier_hash = [template.seed; 32];
                if let Some(last) = nullifier_hash.last_mut() {
                    *last = u8::try_from(input_index).unwrap_or(u8::MAX);
                }
                InputUtxo {
                    nullifier_hash,
                    tree_index: 0,
                }
            })
            .collect();
        let outputs = (0..template.shape.n_outputs())
            .map(|_| TransactOutput {
                utxo_hash: [0; 32],
                owner_tag: OwnerTag::Inline([0; 32]),
                data: Some(vec![0; data_len]),
            })
            .collect();
        let n_inputs = u8::try_from(template.shape.n_inputs()).unwrap_or(u8::MAX);
        let n_outputs = u8::try_from(template.shape.n_outputs()).unwrap_or(u8::MAX);
        let public_slots = u8::try_from(N_PUBLIC_SLOTS).unwrap_or(u8::MAX);
        let input_trees: Vec<Address> = std::iter::once(self.tree)
            .chain(template.extra_input_tree)
            .collect();
        let tree_contexts = input_trees
            .iter()
            .map(|_| TreeContext {
                utxo_tree_root_index: 0,
                nullifier_tree_root_index: 0,
            })
            .collect();
        let transact = Transact {
            payer: self.market_maker,
            input_trees,
            output_tree: self.tree,
            owner_signers: template.owner_signer.into_iter().collect(),
            interface_transfer_accounts: template
                .withdrawal
                .map(TransactInterfaceTransferAccounts::SplWithdrawal)
                .into_iter()
                .collect(),
            data: TransactIxData {
                proof: TransactProof::zeroed(),
                expiry_unix_ts: 0,
                private_tx_hash: [0; 32],
                circuit: CircuitId::ConfidentialEddsa(n_inputs, n_outputs, public_slots),
                inputs,
                interface_transfers: template
                    .withdrawal
                    .map(|_| InterfaceTransfer::SplWithdrawal {
                        amount: 1,
                        spl_interface_bump: 0,
                    })
                    .into_iter()
                    .collect(),
                data_hash: None,
                ring_data_hash: None,
                tx_viewing_pk: [0; 33],
                salt: [0; 16],
                outputs,
                messages: Vec::new(),
                tree_contexts,
            },
        };
        transact.instruction()
    }
}

fn placeholder_withdrawal() -> TransactSplWithdrawalAccounts {
    TransactSplWithdrawalAccounts {
        mint: PLACEHOLDER_MINT,
        spl_interface: pda::spl_interface(&PLACEHOLDER_MINT),
        user_token_account: PLACEHOLDER_TOKEN_ACCOUNT,
        token_program: pda::spl_token_program_id(),
    }
}

fn check_compute_units() -> Result<(), BudgetError> {
    let units = SWAP_COMPUTE_BUDGET.cu_limit;
    if units > MAX_COMPUTE_UNITS {
        return Err(BudgetError::ComputeBudgetExceeded {
            units,
            max: MAX_COMPUTE_UNITS,
        });
    }
    Ok(())
}

/// The byte length of one encrypted output, measured by encrypting a dummy
/// plaintext; the confidential encoding has a fixed length.
fn output_data_len() -> Result<usize, BudgetError> {
    let throwaway = ViewingKey::new();
    Ok(Confidential::encode_plaintext(
        &ConfidentialOutputPlaintext {
            asset_id: SOL_ASSET_ID,
            amount: 0,
            blinding: [0; 32],
            ring_program_id: None,
            data: Data::default(),
        },
        [0; 32],
        &ConfidentialEncode {
            recipient_pubkey: throwaway.pubkey(),
            tx: throwaway,
            salt: [0; 16],
            slot_index: 0,
        },
    )?
    .data
    .len())
}

#[cfg(test)]
mod tests {
    use super::*;

    const MARKET_MAKER: Address = Address::new_from_array([1; 32]);
    const TREE: Address = Address::new_from_array([2; 32]);

    /// Consolidation sizing follows the requested output count, not
    /// `USER_OUTPUTS`: one output sizes like the narrowest shape's output
    /// count, and more outputs than the widest shape fails with
    /// `NoSupportedShape`.
    #[test]
    fn consolidate_outputs_are_not_clamped_to_user_outputs() -> Result<(), BudgetError> {
        let budget = SwapBudget::new(MARKET_MAKER, TREE, 1)?;
        let widest_output_shape = SPP_SUPPORTED_SHAPES
            .into_iter()
            .map(|shape| shape.n_outputs())
            .max()
            .unwrap_or(0);
        let one = budget.max_consolidate_inputs_with(1, placeholder_withdrawal(), &[])?;
        let narrowest = SPP_SUPPORTED_SHAPES
            .into_iter()
            .map(|shape| shape.n_outputs())
            .min()
            .unwrap_or(0);
        let at_narrowest =
            budget.max_consolidate_inputs_with(narrowest, placeholder_withdrawal(), &[])?;
        assert_eq!(
            one, at_narrowest,
            "inputs at one output: got {one:?}, want {at_narrowest:?} as at {narrowest} outputs"
        );
        let too_wide = widest_output_shape + 1;
        let refused = budget.max_consolidate_inputs_with(too_wide, placeholder_withdrawal(), &[]);
        assert!(
            matches!(
                refused,
                Err(BudgetError::NoSupportedShape { inputs: 1, outputs }) if outputs == too_wide
            ),
            "got {refused:?}, want NoSupportedShape {{ inputs: 1, outputs: {too_wide} }}"
        );
        Ok(())
    }
}
