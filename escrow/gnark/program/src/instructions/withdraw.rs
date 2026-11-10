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
        shared::{check_after_window, u64_right_align, EscrowAuthority, IxData},
        verifier::{verify_groth16, CompressedGroth16Proof},
    },
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, SchemaRead, SchemaWrite)]
pub struct WithdrawProof {
    pub proof_a: [u8; 32],
    pub proof_b: [u8; 64],
    pub proof_c: [u8; 32],
}

/// The withdraw arguments the program reads ahead of the transact bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, SchemaRead, SchemaWrite)]
pub struct WithdrawArgs {
    pub proof: WithdrawProof,
    /// See [`WithdrawIxData::unlock_timestamp`].
    pub unlock_timestamp: u64,
    /// See [`WithdrawIxData::authority_bump`].
    pub authority_bump: u8,
}

/// The withdraw instruction data as a client writes it: [`WithdrawArgs`],
/// then the transact data, which the program reads in place ([`IxData`]) and
/// forwards to SPP unchanged.
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
    /// The bump of the creator's escrow-authority PDA. The SDK passes the
    /// canonical bump; see [`EscrowAuthority::new`].
    pub authority_bump: u8,
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
    // The escrow utxo is owned by this creator's escrow-authority PDA, which
    // the program signs for.
    let creator = *iter.next_signer("creator")?.address();
    let owner_pk_field =
        solana_owner_identity(creator.as_array()).map_err(TimelockEscrowError::from)?;

    // The program reads the transact data in place and forwards the client's
    // bytes to SPP unchanged.
    let IxData {
        args:
            WithdrawArgs {
                proof,
                unlock_timestamp,
                authority_bump,
            },
        transact,
        transact_bytes,
    } = IxData::<WithdrawArgs>::parse(data)?;
    let escrow_authority = EscrowAuthority::new(&creator, authority_bump)?;

    let clock = Clock::get()?;
    check_after_window(clock.unix_timestamp, unlock_timestamp)?;

    verify_withdraw_proof(
        &proof,
        unlock_timestamp,
        &owner_pk_field,
        transact.private_tx_hash,
    )?;

    let spp_accounts = iter.remaining()?;
    escrow_authority.cpi_spp_transact_signed(spp_accounts, transact_bytes)
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
