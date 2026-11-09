use light_program_profiler::profile;
use pinocchio::{
    error::ProgramError,
    sysvars::{clock::Clock, Sysvar},
    AccountView, ProgramResult,
};
use spr_account_checks::AccountIterator;
use spr_hasher::primitives::solana_owner_identity;
use spr_hasher::{Hasher, Poseidon};
use spr_interface::instruction::instruction_data::transact::TransactIxData;
use wincode::{SchemaRead, SchemaWrite};

use crate::{
    error::TimelockEscrowError,
    instructions::{
        shared::{check_after_window, cpi_spp_transact_signed, u64_right_align},
        verifier::{verify_groth16, CompressedGroth16Proof},
    },
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, SchemaRead, SchemaWrite)]
pub struct WithdrawProof {
    pub proof_a: [u8; 32],
    pub proof_b: [u8; 64],
    pub proof_c: [u8; 32],
}

#[derive(Clone, Debug, PartialEq, Eq, SchemaRead, SchemaWrite)]
pub struct WithdrawIxData {
    pub proof: WithdrawProof,
    /// The committed escrow `unlock` timestamp the withdraw proof reveals as a
    /// public input. Separate from `transact.expiry_unix_ts` (the SPP relayer
    /// deadline): withdraw requires `now > unlock`, which is necessarily in the
    /// past, whereas SPP rejects a `transact` whose `expiry_unix_ts` is in the
    /// past. The escrow's committed terms hash includes `unlock`; the proof
    /// recomputes it.
    pub unlock_timestamp: u64,
    pub transact: TransactIxData,
}

pub struct WithdrawPublicInput<'a> {
    pub private_tx_hash: &'a [u8; 32],
    pub unlock: u64,
    pub owner_pk_field: &'a [u8; 32],
}

impl WithdrawPublicInput<'_> {
    pub fn hash(&self) -> Result<[u8; 32], ProgramError> {
        Poseidon::hashv(&[
            self.private_tx_hash.as_slice(),
            u64_right_align(self.unlock).as_slice(),
            self.owner_pk_field.as_slice(),
        ])
        .map_err(|_| TimelockEscrowError::HashingFailed.into())
    }
}

#[inline(never)]
#[profile]
pub fn process_withdraw_ix(accounts: &mut [AccountView], data: &[u8]) -> ProgramResult {
    let mut iter = AccountIterator::new(accounts);
    iter.next_signer_mut("caller")?;
    // The creator signs the withdraw; the withdraw proof recomputes the
    // escrow's committed owner_hash from this pubkey (owner_pk_field), so only
    // the creator can withdraw and the creator knows the refund blinding it
    // chose. The identity is tagged as a Solana key, like the owner hash the
    // escrow terms commit.
    let owner_pk_field = solana_owner_identity(iter.next_signer("creator")?.address().as_array())
        .map_err(TimelockEscrowError::from)?;

    let WithdrawIxData {
        proof,
        unlock_timestamp,
        transact,
    } = wincode::deserialize_exact(data)
        .map_err(|_| TimelockEscrowError::InvalidInstructionData)?;

    let clock = Clock::get()?;
    check_after_window(clock.unix_timestamp, unlock_timestamp)?;

    verify_withdraw_proof(
        &proof,
        unlock_timestamp,
        &owner_pk_field,
        &transact.private_tx_hash,
    )?;

    let transact_bytes = transact
        .serialize()
        .map_err(|_| TimelockEscrowError::InvalidInstructionData)?;
    let spp_accounts = iter.remaining()?;
    cpi_spp_transact_signed(spp_accounts, &transact_bytes)
}

/// Verifies the withdraw proof against the escrow's `unlock` timestamp, the
/// creator's owner identity and the transaction's `private_tx_hash`: the
/// public-input hash, then the Groth16 verification.
#[inline(never)]
#[profile]
pub fn verify_withdraw_proof(
    proof: &WithdrawProof,
    unlock: u64,
    owner_pk_field: &[u8; 32],
    private_tx_hash: &[u8; 32],
) -> ProgramResult {
    verify_groth16(
        CompressedGroth16Proof {
            a: &proof.proof_a,
            b: &proof.proof_b,
            c: &proof.proof_c,
            commitment: None,
        },
        WithdrawPublicInput {
            private_tx_hash,
            unlock,
            owner_pk_field,
        }
        .hash()?,
        &crate::verifying_keys::withdraw::VERIFYINGKEY,
    )
}
