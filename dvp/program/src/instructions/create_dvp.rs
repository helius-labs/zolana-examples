use light_program_profiler::profile;
use pinocchio::{error::ProgramError, AccountView, ProgramResult};
use wincode::{SchemaRead, SchemaWrite};
use zolana_account_checks::AccountIterator;
use zolana_hasher::{primitives::solana_owner_identity, Hasher, Poseidon};
use zolana_interface::instruction::instruction_data::transact::TransactIxData;

use crate::{
    error::DvpError,
    instructions::{
        shared::{cpi_spp_transact_signed, dvp_authority, dvp_authority_owner_hash},
        verifier::{verify_groth16, CompressedGroth16Proof},
    },
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, SchemaRead, SchemaWrite)]
pub struct CreateDvpProof {
    pub proof_a: [u8; 32],
    pub proof_b: [u8; 64],
    pub proof_c: [u8; 32],
}

#[derive(Clone, Debug, PartialEq, Eq, SchemaRead, SchemaWrite)]
pub struct CreateDvpIxData {
    /// The terms hash. Seeds the PDA and the address slot.
    pub dvp_id: [u8; 32],
    /// Canonical bump of `dvp_authority` (`find_program_address`).
    pub bump: u8,
    pub create_proof: CreateDvpProof,
    pub spp: TransactIxData,
}

/// The create proof's single public input. The program supplies every
/// element: `private_tx_hash` from the SPP transact it CPIs, `creator_id` from
/// the creator signer, `dvp_id` from the instruction data, and the owner hash
/// from the PDA it derives from both.
pub struct CreateDvpPublicInput<'a> {
    pub private_tx_hash: &'a [u8; 32],
    /// `solana_owner_identity(creator)`.
    pub creator_id: &'a [u8; 32],
    pub dvp_id: &'a [u8; 32],
    /// `Poseidon(solana_owner_identity(dvp_authority), ZERO_NULLIFIER_PUBKEY)`.
    pub dvp_authority_owner_hash: &'a [u8; 32],
}

impl CreateDvpPublicInput<'_> {
    pub fn hash(&self) -> Result<[u8; 32], ProgramError> {
        Poseidon::hashv(&[
            self.private_tx_hash.as_slice(),
            self.creator_id.as_slice(),
            self.dvp_id.as_slice(),
            self.dvp_authority_owner_hash.as_slice(),
        ])
        .map_err(|_| DvpError::HashingFailed.into())
    }
}

/// Creates a DvP: the SPP transact consumes the `(dvp_authority, dvp_id)`
/// address and writes the DvP UTXO committing the terms. The create proof
/// binds that transact (`private_tx_hash`) to the signing creator, the
/// `dvp_id` and the PDA's owner hash.
///
/// The `dvp_authority` PDA is not a separate account here: it is forwarded
/// as the last SPP account, the owner-signer run, as in escrow and swap.
/// The program does not check the transact's messages; the parties'
/// terms ciphertexts are only bound by the recipients' own checks.
#[inline(never)]
#[profile]
pub fn process_create_dvp_ix(accounts: &mut [AccountView], data: &[u8]) -> ProgramResult {
    let mut iter = AccountIterator::new(accounts);
    iter.next_signer_mut("payer")?;
    // The creator signs; its identity is a public input, so only the
    // creator the terms name can create them, and the PDA seeds include it.
    let creator = *iter.next_signer("creator")?.address();
    let creator_id = solana_owner_identity(creator.as_array()).map_err(DvpError::from)?;

    let CreateDvpIxData {
        dvp_id,
        bump,
        create_proof,
        spp,
    } = wincode::deserialize_exact(data).map_err(|_| DvpError::InvalidInstructionData)?;

    let dvp_authority = dvp_authority(&creator, &dvp_id, bump)?;
    let dvp_authority_owner_hash = dvp_authority_owner_hash(&dvp_authority)?;

    verify_groth16(
        CompressedGroth16Proof {
            a: &create_proof.proof_a,
            b: &create_proof.proof_b,
            c: &create_proof.proof_c,
            commitment: None,
        },
        CreateDvpPublicInput {
            private_tx_hash: &spp.private_tx_hash,
            creator_id: &creator_id,
            dvp_id: &dvp_id,
            dvp_authority_owner_hash: &dvp_authority_owner_hash,
        }
        .hash()?,
        &crate::verifying_keys::create_dvp::VERIFYINGKEY,
    )?;

    let spp_bytes = spp
        .serialize()
        .map_err(|_| DvpError::InvalidInstructionData)?;
    let spp_accounts = iter.remaining()?;
    cpi_spp_transact_signed(
        spp_accounts,
        &spp_bytes,
        &dvp_authority,
        &creator,
        &dvp_id,
        bump,
    )
}
