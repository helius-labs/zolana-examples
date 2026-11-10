use light_program_profiler::profile;
use pinocchio::{error::ProgramError, AccountView, ProgramResult};
use spr_account_checks::AccountIterator;
use spr_hasher::{primitives::solana_owner_identity, Hasher, Poseidon};
use spr_interface::instruction::instruction_data::transact::TransactIxData;
use wincode::{SchemaRead, SchemaWrite};

use crate::{
    error::TimelockEscrowError,
    instructions::{
        shared::{EscrowAuthority, IxData},
        verifier::{verify_groth16, CompressedGroth16Proof},
    },
    verifying_keys::escrow,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, SchemaRead, SchemaWrite)]
pub struct EscrowProof {
    pub proof_a: [u8; 32],
    pub proof_b: [u8; 64],
    pub proof_c: [u8; 32],
}

/// The escrow arguments the program reads ahead of the transact bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, SchemaRead, SchemaWrite)]
pub struct EscrowArgs {
    pub proof: EscrowProof,
    /// See [`EscrowIxData::authority_bump`].
    pub authority_bump: u8,
}

/// The escrow instruction data as a client writes it: [`EscrowArgs`], then
/// the transact data, which the program reads in place ([`IxData`]) and
/// forwards to SPP unchanged.
#[derive(Clone, Debug, PartialEq, Eq, SchemaRead, SchemaWrite)]
pub struct EscrowIxData {
    pub proof: EscrowProof,
    /// The bump of the creator's escrow-authority PDA. The SDK passes the
    /// canonical bump; see [`EscrowAuthority::new`].
    pub authority_bump: u8,
    pub transact: TransactIxData,
}

/// The escrow circuit's single public input: the transaction's
/// `private_tx_hash`, the owner hash the escrow utxo must carry (the
/// creator's escrow-authority PDA) and the creator's owner identity, which
/// the escrow terms' owner hash, and so the source owner, must be built from.
pub struct EscrowPublicInput<'a> {
    pub private_tx_hash: &'a [u8; 32],
    pub escrow_owner_hash: &'a [u8; 32],
    pub owner_pk_field: &'a [u8; 32],
}

impl EscrowPublicInput<'_> {
    pub fn hash(&self) -> Result<[u8; 32], ProgramError> {
        Poseidon::hashv(&[
            self.private_tx_hash.as_slice(),
            self.escrow_owner_hash.as_slice(),
            self.owner_pk_field.as_slice(),
        ])
        .map_err(|_| TimelockEscrowError::HashingFailed.into())
    }
}

#[inline(never)]
#[profile]
pub fn process_escrow_ix(accounts: &mut [AccountView], data: &[u8]) -> ProgramResult {
    let mut iter = AccountIterator::new(accounts);
    // The creator signs the escrow. The proof binds the escrow terms and the
    // spent source to its owner identity, so the creator funds the escrow
    // from its own balance and signs the SPP transact as the source's owner
    // signer, and the creator who withdraws later derives the same PDA. Its
    // escrow-authority PDA owns the escrow utxo; the program signs for it
    // only because SPP requires the owner of the data-bearing escrow output
    // to sign.
    let creator = *iter.next_signer("creator")?.address();
    let owner_pk_field =
        solana_owner_identity(creator.as_array()).map_err(TimelockEscrowError::from)?;

    // The program reads the transact data in place and forwards the client's
    // bytes to SPP unchanged.
    let IxData {
        args: EscrowArgs {
            proof,
            authority_bump,
        },
        transact,
        transact_bytes,
    } = IxData::<EscrowArgs>::parse(data)?;
    let escrow_authority = EscrowAuthority::new(&creator, authority_bump)?;

    verify_escrow_proof(
        &proof,
        transact.private_tx_hash,
        &escrow_authority.owner_hash()?,
        &owner_pk_field,
    )?;

    let spp_accounts = iter.remaining()?;
    escrow_authority.cpi_spp_transact_signed(spp_accounts, transact_bytes)
}

/// Verifies the escrow proof against the transaction's `private_tx_hash`,
/// the creator's escrow-authority owner hash and the creator's owner
/// identity: the public-input hash, then the Groth16 verification.
#[inline(never)]
#[profile]
pub fn verify_escrow_proof(
    proof: &EscrowProof,
    private_tx_hash: &[u8; 32],
    escrow_owner_hash: &[u8; 32],
    owner_pk_field: &[u8; 32],
) -> ProgramResult {
    verify_groth16(
        CompressedGroth16Proof {
            a: &proof.proof_a,
            b: &proof.proof_b,
            c: &proof.proof_c,
            commitment: None,
        },
        EscrowPublicInput {
            private_tx_hash,
            escrow_owner_hash,
            owner_pk_field,
        }
        .hash()?,
        &escrow::VERIFYINGKEY,
    )
}
