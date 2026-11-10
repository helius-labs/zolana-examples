use borsh::{BorshDeserialize, BorshSerialize};
use light_program_profiler::profile;
use pinocchio::{AccountView, ProgramResult};
use spr_account_checks::AccountIterator;
use spr_interface::instruction::instruction_data::transact::TransactIxData;
use wincode::{SchemaRead, SchemaWrite};

use crate::{error::SwapError, instructions::shared::OrderAuthority, verifier::make};

#[derive(Clone, Copy, Debug, PartialEq, Eq, BorshDeserialize, BorshSerialize)]
pub struct MarkerData {
    pub order_utxo_hash: [u8; 32],
    pub maker_pubkey: [u8; 32],
}

pub use spr_program::CompressedProof as MakeProof;

#[derive(Clone, Debug, PartialEq, Eq, SchemaRead, SchemaWrite)]
pub struct MakeIxData {
    pub proof: MakeProof,
    /// The viewing key of the order UTXO's address, the taker's, which the
    /// order output is encrypted to. The make proof's public `order_owner`
    /// is the maker's order-authority PDA with this viewing key.
    pub viewing_pubkey: [u8; 33],
    pub transact: TransactIxData,
}

const ORDER_OUTPUT_INDEX: usize = 1;

/// 1. Check the maker signer and derive its order-authority PDA.
/// 2. Decode the instruction.
/// 3. Verify the make proof against the transaction, the maker and the order
///    UTXO's owner, the maker's order-authority PDA.
/// 4. Write the marker message.
/// 5. Invoke SPP with the order-authority PDA signer, which authorizes the
///    order output; the maker signs for the source it owns.
#[inline(never)]
#[profile]
pub fn process_make_ix(accounts: &mut [AccountView], data: &[u8]) -> ProgramResult {
    // 1. Check the maker signer. The make proof checks the source owner's
    // identity against this pubkey, so the marker names the order's maker.
    let mut iter = AccountIterator::new(accounts);
    let maker_pubkey = *iter.next_signer_mut("payer")?.address().as_array();
    let order_authority = OrderAuthority::find(&maker_pubkey);

    // 2. Decode the instruction. Unlike take and cancel, make decodes the
    // owned transact data: step 4 writes the marker into a message, which
    // changes that message's length mid-buffer, so the program re-serializes
    // the transact data instead of forwarding the client's bytes.
    let MakeIxData {
        proof,
        viewing_pubkey,
        mut transact,
    } = wincode::deserialize_exact(data).map_err(|_| SwapError::InvalidInstructionData)?;

    // 3. Verify the make proof against the transaction, the maker and the
    // order UTXO's owner. SPP accepts a data-bearing output owned by any
    // owner signer, the maker included; the proof binds it to the PDA.
    let public_inputs = order_authority.make_public_inputs(&viewing_pubkey)?;
    verify_make_proof(&proof, public_inputs, &transact.private_tx_hash)?;

    // 4. Write the marker message.
    let order_utxo_hash = transact
        .outputs
        .get(ORDER_OUTPUT_INDEX)
        .ok_or(SwapError::InvalidInstructionData)?
        .utxo_hash;
    let [marker_message] = transact.messages.as_mut_slice() else {
        return Err(SwapError::InvalidMarkerMessage.into());
    };
    if !marker_message.data.is_empty() {
        return Err(SwapError::MarkerDataNotEmpty.into());
    }
    let marker = MarkerData {
        order_utxo_hash,
        maker_pubkey,
    };
    marker_message.data = borsh::to_vec(&marker).map_err(|_| SwapError::InvalidInstructionData)?;
    let transact_bytes = transact
        .serialize()
        .map_err(|_| SwapError::InvalidInstructionData)?;

    // 5. Invoke SPP with the maker's order-authority PDA signer.
    let spp_accounts = iter.remaining()?;
    order_authority.cpi_spp_transact_signed(spp_accounts, &transact_bytes)
}

/// Hashes the make proof's public inputs and verifies the proof.
#[inline(never)]
#[profile]
fn verify_make_proof(
    proof: &MakeProof,
    public_inputs: make::PublicInputs,
    private_tx_hash: &[u8; 32],
) -> ProgramResult {
    make::Verifier {
        proof,
        public_inputs,
        private_tx_hash,
        transfers: &[],
    }
    .verify()
    .map_err(SwapError::from)?;
    Ok(())
}
