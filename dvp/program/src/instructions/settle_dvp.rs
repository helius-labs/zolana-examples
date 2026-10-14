use light_program_profiler::profile;
use pinocchio::{
    error::ProgramError,
    sysvars::{clock::Clock, Sysvar},
    AccountView, Address, ProgramResult,
};
use wincode::{SchemaRead, SchemaWrite};
use zolana_account_checks::AccountIterator;
use zolana_hasher::{primitives::solana_owner_identity, Hasher, Poseidon};
use zolana_interface::instruction::instruction_data::transact::TransactIxData;

use crate::{
    error::DvpError,
    instructions::{
        shared::{
            check_settle_timestamp, cpi_spp_transact_signed, dvp_authority,
            dvp_authority_owner_hash, u64_right_align,
        },
        verifier::{verify_groth16, CompressedGroth16Proof},
    },
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, SchemaRead, SchemaWrite)]
pub struct SettleDvpProof {
    pub proof_a: [u8; 32],
    pub proof_b: [u8; 64],
    pub proof_c: [u8; 32],
}

#[derive(Clone, Debug, PartialEq, Eq, SchemaRead, SchemaWrite)]
pub struct SettleDvpIxData {
    /// The terms' creator. With `dvp_id` and `bump` it seeds the PDA; the
    /// settle proof checks it against the terms.
    pub creator: Address,
    pub dvp_id: [u8; 32],
    /// Canonical bump of `dvp_authority` (`find_program_address`).
    pub bump: u8,
    /// Time the settle proof checks the terms' settlement window against.
    /// The program requires `now - TIME_WINDOW <= current_timestamp <= now`
    /// against the Clock, and a non-negative value (D5).
    pub current_timestamp: i64,
    pub settle_proof: SettleDvpProof,
    pub spp: TransactIxData,
}

/// The settle proof's single public input. The program supplies every
/// element: `private_tx_hash` from the SPP transact it CPIs, `creator_id`,
/// `dvp_id` and `current_timestamp` from the instruction data, the owner hash
/// from the PDA it derives, and `settlement_authority_id` from the signer.
pub struct SettleDvpPublicInput<'a> {
    pub private_tx_hash: &'a [u8; 32],
    /// `solana_owner_identity(creator)`.
    pub creator_id: &'a [u8; 32],
    pub dvp_id: &'a [u8; 32],
    /// `Poseidon(solana_owner_identity(dvp_authority), ZERO_NULLIFIER_PUBKEY)`.
    pub dvp_authority_owner_hash: &'a [u8; 32],
    /// `solana_owner_identity(settlement_authority)`.
    pub settlement_authority_id: &'a [u8; 32],
    /// The instruction's timestamp, already checked against the clock with
    /// `check_settle_timestamp`. It enters the hash as a right-aligned u64,
    /// so a negative value is refused (D5).
    pub current_timestamp: i64,
}

impl SettleDvpPublicInput<'_> {
    pub fn hash(&self) -> Result<[u8; 32], ProgramError> {
        let current_timestamp =
            u64::try_from(self.current_timestamp).map_err(|_| DvpError::TimestampOutOfWindow)?;
        Poseidon::hashv(&[
            self.private_tx_hash.as_slice(),
            self.creator_id.as_slice(),
            self.dvp_id.as_slice(),
            self.dvp_authority_owner_hash.as_slice(),
            self.settlement_authority_id.as_slice(),
            u64_right_align(current_timestamp).as_slice(),
        ])
        .map_err(|_| DvpError::HashingFailed.into())
    }
}

/// Settles the DvP: the 3x4 SPP transact spends the DvP UTXO and both
/// deposits, all owned by `dvp_authority`, and pays the delivery leg to the
/// buyer, the payment leg to the seller, and each leg's surplus to its
/// funder's refund destination. Spending the DvP UTXO closes the DvP, so a
/// second settle (or an abort) fails on its nullifier.
///
/// The settle proof checks `current_timestamp` against the terms' window
/// (`earliest_settlement_timestamp <= current_timestamp <= expiry_timestamp`),
/// which stays private. The program ties `current_timestamp` to the Clock:
/// it may trail `now` by at most `TIME_WINDOW` seconds, so a proof made
/// against a recent time still lands, and a settle can land up to
/// `TIME_WINDOW` after expiry. It may not be ahead of `now`, and it must be
/// non-negative (D5). The PDA is forwarded as the last SPP account, the
/// owner-signer run (D8).
#[inline(never)]
#[profile]
pub fn process_settle_dvp_ix(accounts: &mut [AccountView], data: &[u8]) -> ProgramResult {
    let mut iter = AccountIterator::new(accounts);
    iter.next_signer_mut("payer")?;
    // Only the terms' settlement authority may settle; its identity is a
    // public input the settle proof checks against the terms.
    let settlement_authority = iter.next_signer("settlement_authority")?.address();
    let settlement_authority_id =
        solana_owner_identity(settlement_authority.as_array()).map_err(DvpError::from)?;

    let SettleDvpIxData {
        creator,
        dvp_id,
        bump,
        current_timestamp,
        settle_proof,
        spp,
    } = wincode::deserialize_exact(data).map_err(|_| DvpError::InvalidInstructionData)?;
    let now = Clock::get()?.unix_timestamp;
    check_settle_timestamp(now, current_timestamp)?;
    let creator_id = solana_owner_identity(creator.as_array()).map_err(DvpError::from)?;

    let dvp_authority = dvp_authority(&creator, &dvp_id, bump)?;
    let dvp_authority_owner_hash = dvp_authority_owner_hash(&dvp_authority)?;

    verify_groth16(
        CompressedGroth16Proof {
            a: &settle_proof.proof_a,
            b: &settle_proof.proof_b,
            c: &settle_proof.proof_c,
            commitment: None,
        },
        SettleDvpPublicInput {
            private_tx_hash: &spp.private_tx_hash,
            creator_id: &creator_id,
            dvp_id: &dvp_id,
            dvp_authority_owner_hash: &dvp_authority_owner_hash,
            settlement_authority_id: &settlement_authority_id,
            current_timestamp,
        }
        .hash()?,
        &crate::verifying_keys::settle_dvp::VERIFYINGKEY,
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
