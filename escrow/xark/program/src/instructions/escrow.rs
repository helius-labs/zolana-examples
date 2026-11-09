use light_program_profiler::profile;
use pinocchio::{AccountView, ProgramResult};
use spr_account_checks::AccountIterator;
use spr_hasher::primitives::solana_owner_identity;
use spr_interface::instruction::instruction_data::transact::TransactIxData;
use wincode::{SchemaRead, SchemaWrite};

use crate::{
    error::TimelockEscrowError,
    instructions::shared::{cpi_spp_transact_signed, IxData},
    verifier::escrow,
};

/// The escrow proof: compressed Groth16 A, B and C (128 bytes).
pub use spr_program::CompressedProof as EscrowProof;

/// The escrow instruction data as a client writes it. The program reads the
/// proof, then the transact bytes in place ([`IxData`]), and forwards those
/// bytes to SPP unchanged.
#[derive(Clone, Debug, PartialEq, Eq, SchemaRead, SchemaWrite)]
pub struct EscrowIxData {
    pub proof: EscrowProof,
    pub transact: TransactIxData,
}

/// 1. Check the creator signs.
/// 2. Decode the instruction.
/// 3. Verify the escrow proof against the creator and the transaction's
///    `private_tx_hash`.
/// 4. Invoke SPP with the escrow-authority PDA signer, which the escrow UTXO's
///    data requires.
#[inline(never)]
#[profile]
pub fn process_escrow_ix(accounts: &mut [AccountView], data: &[u8]) -> ProgramResult {
    // 1. Check the creator signs. The escrow proof checks the source owner
    // against this pubkey (owner_pk_field), tagged as a Solana key like the
    // owner the escrow terms commit.
    let mut iter = AccountIterator::new(accounts);
    let owner_pk_field =
        solana_owner_identity(iter.next_signer_mut("creator")?.address().as_array())
            .map_err(TimelockEscrowError::from)?;

    // 2. Decode the instruction.
    let IxData {
        args: proof,
        transact,
        transact_bytes,
    } = IxData::<EscrowProof>::parse(data)?;

    // 3. Verify the escrow proof against the creator and the transaction's
    // `private_tx_hash`.
    verify_escrow_proof(&proof, owner_pk_field, transact.private_tx_hash)?;

    // 4. Invoke SPP with the escrow-authority PDA signer, which the escrow
    // UTXO's data requires.
    let spp_accounts = iter.remaining()?;
    cpi_spp_transact_signed(spp_accounts, transact_bytes)
}

/// Verifies the escrow proof against the creator's owner identity and the
/// transaction's `private_tx_hash`: the public-input hash, then the Groth16
/// verification.
#[inline(never)]
#[profile]
pub fn verify_escrow_proof(
    proof: &EscrowProof,
    owner_pk_field: [u8; 32],
    private_tx_hash: &[u8; 32],
) -> ProgramResult {
    escrow::Verifier {
        proof,
        public_inputs: escrow::PublicInputs { owner_pk_field },
        private_tx_hash,
        transfers: &[],
    }
    .verify()
    .map_err(TimelockEscrowError::from)?;
    Ok(())
}
