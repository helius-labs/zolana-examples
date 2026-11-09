use light_program_profiler::profile;
use pinocchio::{AccountView, ProgramResult};
use spr_account_checks::AccountIterator;
use spr_interface::instruction::instruction_data::transact::TransactIxData;
use wincode::{SchemaRead, SchemaWrite};

use crate::{
    error::TimelockEscrowError, instructions::shared::cpi_spp_transact_signed, verifier::escrow,
};

/// The escrow proof: compressed Groth16 A, B and C (128 bytes).
pub use spr_program::CompressedProof as EscrowProof;

#[derive(Clone, Debug, PartialEq, Eq, SchemaRead, SchemaWrite)]
pub struct EscrowIxData {
    pub proof: EscrowProof,
    pub transact: TransactIxData,
}

/// 1. Check the creator signs.
/// 2. Decode the instruction.
/// 3. Verify the escrow proof against the transaction's `private_tx_hash`.
/// 4. Invoke SPP with the escrow-authority PDA signer.
#[inline(never)]
#[profile]
pub fn process_escrow_ix(accounts: &mut [AccountView], data: &[u8]) -> ProgramResult {
    // 1. Check the creator signs.
    let mut iter = AccountIterator::new(accounts);
    iter.next_signer_mut("creator")?;

    // 2. Decode the instruction.
    let EscrowIxData { proof, transact } = wincode::deserialize_exact(data)
        .map_err(|_| TimelockEscrowError::InvalidInstructionData)?;

    // 3. Verify the escrow proof against the transaction's `private_tx_hash`.
    verify_escrow_proof(&proof, &transact.private_tx_hash)?;

    // 4. Invoke SPP with the escrow-authority PDA signer.
    let transact_bytes = transact
        .serialize()
        .map_err(|_| TimelockEscrowError::InvalidInstructionData)?;
    let spp_accounts = iter.remaining()?;
    cpi_spp_transact_signed(spp_accounts, &transact_bytes)
}

/// Verifies the escrow proof against the transaction's `private_tx_hash`:
/// the public-input hash, then the Groth16 verification.
#[inline(never)]
#[profile]
pub fn verify_escrow_proof(proof: &EscrowProof, private_tx_hash: &[u8; 32]) -> ProgramResult {
    escrow::Verifier {
        proof,
        public_inputs: escrow::PublicInputs {},
        private_tx_hash,
        transfers: &[],
    }
    .verify()
    .map_err(TimelockEscrowError::from)?;
    Ok(())
}
