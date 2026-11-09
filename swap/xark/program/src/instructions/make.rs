use borsh::{BorshDeserialize, BorshSerialize};
use light_program_profiler::profile;
use pinocchio::{AccountView, ProgramResult};
use spr_account_checks::AccountIterator;
use spr_interface::instruction::instruction_data::transact::TransactIxData;
use wincode::{SchemaRead, SchemaWrite};

use crate::{error::SwapError, instructions::shared::cpi_spp_transact_signed, verifier::make};

#[derive(Clone, Copy, Debug, PartialEq, Eq, BorshDeserialize, BorshSerialize)]
pub struct MarkerData {
    pub order_utxo_hash: [u8; 32],
    pub maker_pubkey: [u8; 32],
}

pub use spr_program::CompressedProof as MakeProof;

#[derive(Clone, Debug, PartialEq, Eq, SchemaRead, SchemaWrite)]
pub struct MakeIxData {
    pub proof: MakeProof,
    pub transact: TransactIxData,
}

const ORDER_OUTPUT_INDEX: usize = 1;

/// 1. Check the maker signer.
/// 2. Decode the instruction.
/// 3. Verify the make proof against the transaction.
/// 4. Write the marker message.
/// 5. Invoke SPP with the order-authority PDA signer.
#[inline(never)]
#[profile]
pub fn process_make_ix(accounts: &mut [AccountView], data: &[u8]) -> ProgramResult {
    // 1. Check the maker signer.
    let mut iter = AccountIterator::new(accounts);
    let maker_pubkey = *iter.next_signer_mut("payer")?.address().as_array();

    // 2. Decode the instruction.
    let MakeIxData {
        proof,
        mut transact,
    } = wincode::deserialize_exact(data).map_err(|_| SwapError::InvalidInstructionData)?;

    // 3. Verify the make proof against the transaction.
    make::Verifier {
        proof: &proof,
        public_inputs: make::PublicInputs {},
        private_tx_hash: &transact.private_tx_hash,
        transfers: &[],
    }
    .verify()
    .map_err(SwapError::from)?;

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

    // 5. Invoke SPP with the order-authority PDA signer.
    let spp_accounts = iter.remaining()?;
    cpi_spp_transact_signed(spp_accounts, &transact_bytes)
}
