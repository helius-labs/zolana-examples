use pinocchio::{error::ProgramError, AccountView, Address, ProgramResult};
use wincode::{SchemaRead, SchemaWrite};
use zolana_account_checks::AccountIterator;
use zolana_hasher::{primitives::solana_owner_identity, Hasher, Poseidon};
use zolana_interface::instruction::instruction_data::transact::TransactIxData;

use crate::{
    error::DvpError,
    instructions::{
        shared::{
            cpi_spp_transact_signed, dvp_authority, dvp_authority_owner_hash, u64_right_align,
        },
        verifier::{verify_groth16, CompressedGroth16Proof},
    },
};

/// The proof of the abort_dvp circuit, which cancel_dvp and reject_dvp share.
#[derive(Clone, Copy, Debug, PartialEq, Eq, SchemaRead, SchemaWrite)]
pub struct AbortDvpProof {
    pub proof_a: [u8; 32],
    pub proof_b: [u8; 64],
    pub proof_c: [u8; 32],
}

/// The wire layout `CancelDvpIxData` and `RejectDvpIxData` share: both are
/// `{creator, dvp_id, bump, proof, spp}` and differ only in the proof's field
/// name, so the shared processor reads either through this struct. The
/// field docs are on the two public structs.
#[derive(SchemaRead)]
struct AbortDvpIxData {
    creator: Address,
    dvp_id: [u8; 32],
    bump: u8,
    abort_proof: AbortDvpProof,
    spp: TransactIxData,
}

/// The abort proof's single public input. The program supplies every
/// element: `private_tx_hash` from the SPP transact it CPIs, `creator_id` and
/// `dvp_id` from the instruction data, the owner hash from the PDA it
/// derives, `signer_id` from the aborting signer and `mode` from the
/// instruction tag.
pub struct AbortDvpPublicInput<'a> {
    pub private_tx_hash: &'a [u8; 32],
    /// `solana_owner_identity(creator)`.
    pub creator_id: &'a [u8; 32],
    pub dvp_id: &'a [u8; 32],
    /// `Poseidon(solana_owner_identity(dvp_authority), ZERO_NULLIFIER_PUBKEY)`.
    pub dvp_authority_owner_hash: &'a [u8; 32],
    /// `solana_owner_identity(signer)`: the settlement authority in cancel
    /// mode, the seller or the buyer in reject mode.
    pub signer_id: &'a [u8; 32],
    /// `ABORT_MODE_CANCEL` or `ABORT_MODE_REJECT`, which picks the signer
    /// rule the circuit applies.
    pub mode: u64,
}

impl AbortDvpPublicInput<'_> {
    pub fn hash(&self) -> Result<[u8; 32], ProgramError> {
        Poseidon::hashv(&[
            self.private_tx_hash.as_slice(),
            self.creator_id.as_slice(),
            self.dvp_id.as_slice(),
            self.dvp_authority_owner_hash.as_slice(),
            self.signer_id.as_slice(),
            u64_right_align(self.mode).as_slice(),
        ])
        .map_err(|_| DvpError::HashingFailed.into())
    }
}

/// Aborts the DvP, the processor `cancel_dvp` and `reject_dvp` share: the
/// 3x2 SPP transact spends the DvP UTXO and up to two deposits, all owned by
/// `dvp_authority`, and refunds each leg in full to its funder's refund
/// destination (output 0 the seller's delivery leg, output 1 the buyer's
/// payment leg; a leg without a deposit gets a zero-amount refund).
/// Spending the DvP UTXO closes the DvP, so a later settle or abort fails on
/// its nullifier. Deposits that arrive after the abort stay reclaimable.
///
/// `mode` comes from the instruction tag, not the instruction data, and picks
/// the signer rule the abort proof enforces: `ABORT_MODE_CANCEL` requires the
/// terms' settlement authority, `ABORT_MODE_REJECT` the seller or the buyer.
/// The deposits fill input slots 1 and 2 first and padding (random dummies)
/// last; a private leg-order bit says which leg sits in slot 1, so the
/// transact shows neither how many legs were funded nor which.
///
/// As the spec says, there is no expiry check: an abort is valid before or
/// after expiry. The PDA is forwarded as the last SPP account, the
/// owner-signer run.
pub(crate) fn process_abort_dvp(
    accounts: &mut [AccountView],
    data: &[u8],
    mode: u8,
) -> ProgramResult {
    let mut iter = AccountIterator::new(accounts);
    iter.next_signer_mut("payer")?;
    // The aborting signer's identity is a public input; the abort proof
    // checks it against the terms under `mode`'s signer rule.
    let signer = iter.next_signer("signer")?.address();
    let signer_id = solana_owner_identity(signer.as_array()).map_err(DvpError::from)?;

    let AbortDvpIxData {
        creator,
        dvp_id,
        bump,
        abort_proof,
        spp,
    } = wincode::deserialize_exact(data).map_err(|_| DvpError::InvalidInstructionData)?;
    let creator_id = solana_owner_identity(creator.as_array()).map_err(DvpError::from)?;

    let dvp_authority = dvp_authority(&creator, &dvp_id, bump)?;
    let dvp_authority_owner_hash = dvp_authority_owner_hash(&dvp_authority)?;

    verify_groth16(
        CompressedGroth16Proof {
            a: &abort_proof.proof_a,
            b: &abort_proof.proof_b,
            c: &abort_proof.proof_c,
            commitment: None,
        },
        AbortDvpPublicInput {
            private_tx_hash: &spp.private_tx_hash,
            creator_id: &creator_id,
            dvp_id: &dvp_id,
            dvp_authority_owner_hash: &dvp_authority_owner_hash,
            signer_id: &signer_id,
            mode: u64::from(mode),
        }
        .hash()?,
        &crate::verifying_keys::abort_dvp::VERIFYINGKEY,
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
