use borsh::{BorshDeserialize, BorshSerialize};
use light_program_profiler::profile;
use pinocchio::{AccountView, ProgramResult};
use spr_account_checks::AccountIterator;
use spr_interface::instruction::instruction_data::transact::{TransactIxData, TransactIxDataRef};
use wincode::{SchemaRead, SchemaWrite};

use crate::{
    error::SwapError,
    instructions::shared::{IxData, OrderAuthority},
    verifier::make,
};

/// The make transaction's single message: the order UTXO's hash and the
/// maker, Borsh-encoded (64 bytes), which the taker's discovery reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq, BorshDeserialize, BorshSerialize)]
pub struct MarkerData {
    pub order_utxo_hash: [u8; 32],
    pub maker_pubkey: [u8; 32],
}

pub use spr_program::CompressedProof as MakeProof;

/// The make arguments the program reads ahead of the transact bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, SchemaRead, SchemaWrite)]
pub struct MakeArgs {
    pub proof: MakeProof,
    /// See [`MakeIxData::viewing_pubkey`].
    pub viewing_pubkey: [u8; 33],
    /// See [`MakeIxData::authority_bump`].
    pub authority_bump: u8,
}

/// The make instruction data as a client writes it: [`MakeArgs`], then the
/// transact data, which the program reads in place ([`IxData`]) and forwards
/// to SPP unchanged.
#[derive(Clone, Debug, PartialEq, Eq, SchemaRead, SchemaWrite)]
pub struct MakeIxData {
    pub proof: MakeProof,
    /// The viewing key of the order UTXO's address, the taker's, which the
    /// order output is encrypted to. The make proof's public `order_owner`
    /// is the maker's order-authority PDA with this viewing key.
    pub viewing_pubkey: [u8; 33],
    /// The bump of the maker's order-authority PDA; see
    /// [`OrderAuthority::new`].
    pub authority_bump: u8,
    /// Carries exactly one message, the marker, whose data is
    /// [`MarkerData`] of the order output and the maker signer.
    pub transact: TransactIxData,
}

const ORDER_OUTPUT_INDEX: usize = 1;

/// 1. Check the maker signer.
/// 2. Decode the instruction and derive the maker's order-authority PDA.
/// 3. Check the marker message names the order UTXO and the maker.
/// 4. Verify the make proof against the transaction, the maker and the order
///    UTXO's owner, the maker's order-authority PDA.
/// 5. Invoke SPP with the order-authority PDA signer, which authorizes the
///    order output; the maker signs for the source it owns.
#[inline(never)]
#[profile]
pub fn process_make_ix(accounts: &mut [AccountView], data: &[u8]) -> ProgramResult {
    // 1. Check the maker signer. The make proof checks the source owner's
    // identity against this pubkey, so the marker names the order's maker.
    let mut iter = AccountIterator::new(accounts);
    let maker_pubkey = *iter.next_signer_mut("payer")?.address().as_array();

    // 2. Decode the instruction and derive the order-authority PDA.
    let IxData {
        args:
            MakeArgs {
                proof,
                viewing_pubkey,
                authority_bump,
            },
        transact,
        transact_bytes,
    } = IxData::<MakeArgs>::parse(data)?;
    let order_authority = OrderAuthority::new(&maker_pubkey, authority_bump)?;

    // 3. Check the marker message. SPP hashes the messages into the external
    // data its proof covers, so the program checks the client's marker
    // instead of writing it, and forwards the bytes unchanged.
    check_marker(&transact, maker_pubkey)?;

    // 4. Verify the make proof against the transaction, the maker and the
    // order UTXO's owner. SPP accepts a data-bearing output owned by any
    // owner signer, the maker included; the proof binds it to the PDA.
    let public_inputs = order_authority.make_public_inputs(&viewing_pubkey)?;
    verify_make_proof(&proof, public_inputs, transact.private_tx_hash)?;

    // 5. Invoke SPP with the maker's order-authority PDA signer.
    let spp_accounts = iter.remaining()?;
    order_authority.cpi_spp_transact_signed(spp_accounts, transact_bytes)
}

/// The transaction carries exactly one message, whose data is the
/// [`MarkerData`] of the order output's hash and `maker_pubkey`.
fn check_marker(transact: &TransactIxDataRef, maker_pubkey: [u8; 32]) -> ProgramResult {
    let order_utxo_hash = transact
        .outputs
        .get(ORDER_OUTPUT_INDEX)
        .ok_or(SwapError::InvalidInstructionData)?
        .utxo_hash;
    let [marker_message] = transact.messages.as_slice() else {
        return Err(SwapError::InvalidMarkerMessage.into());
    };
    // Compare the Borsh encoding, `order_utxo_hash || maker_pubkey`, in
    // place. Slice equality also checks the lengths, so the data is 64 bytes.
    let (marker_order_utxo_hash, marker_maker_pubkey) = marker_message
        .data
        .split_at_checked(32)
        .ok_or(SwapError::InvalidMarkerData)?;
    if marker_order_utxo_hash != order_utxo_hash.as_slice()
        || marker_maker_pubkey != maker_pubkey.as_slice()
    {
        return Err(SwapError::InvalidMarkerData.into());
    }
    Ok(())
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
