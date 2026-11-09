use light_program_profiler::profile;
use pinocchio::{
    sysvars::{clock::Clock, Sysvar},
    AccountView, ProgramResult,
};
use spr_account_checks::AccountIterator;
use spr_interface::instruction::instruction_data::transact::TransactIxData;
use wincode::{SchemaRead, SchemaWrite};

use crate::{
    error::SwapError,
    instructions::shared::{check_within_window, cpi_spp_transact_signed},
    verifier::take,
};

pub use spr_program::CompressedProof as TakeProof;

#[derive(Clone, Debug, PartialEq, Eq, SchemaRead, SchemaWrite)]
pub struct TakeIxData {
    pub proof: TakeProof,
    pub transact: TransactIxData,
}

/// 1. Check the fee payer.
/// 2. Decode the instruction.
/// 3. Check `now <= expiry`, the expiry being the transact's `expiry_unix_ts`.
/// 4. Verify the take proof against the expiry and the first nullifier.
/// 5. Invoke SPP with the order-authority PDA signer.
#[inline(never)]
#[profile]
pub fn process_take_ix(accounts: &mut [AccountView], data: &[u8]) -> ProgramResult {
    // 1. Check the fee payer.
    let mut iter = AccountIterator::new(accounts);
    iter.next_signer_mut("payer")?;

    // 2. Decode the instruction.
    let TakeIxData { proof, transact } =
        wincode::deserialize_exact(data).map_err(|_| SwapError::InvalidInstructionData)?;

    // 3. Check `now <= expiry`.
    let clock = Clock::get()?;
    check_within_window(clock.unix_timestamp, transact.expiry_unix_ts)?;

    // 4. Verify the take proof.
    let first_nullifier = transact
        .inputs
        .first()
        .ok_or(SwapError::InvalidInstructionData)?
        .nullifier_hash;
    verify_take_proof(
        &proof,
        transact.expiry_unix_ts,
        first_nullifier,
        &transact.private_tx_hash,
    )?;

    // 5. Invoke SPP with the order-authority PDA signer.
    let transact_bytes = transact
        .serialize()
        .map_err(|_| SwapError::InvalidInstructionData)?;
    let spp_accounts = iter.remaining()?;
    cpi_spp_transact_signed(spp_accounts, &transact_bytes)
}

/// Hashes the take proof's public inputs and verifies the proof.
#[inline(never)]
#[profile]
fn verify_take_proof(
    proof: &TakeProof,
    expiry: u64,
    first_nullifier: [u8; 32],
    private_tx_hash: &[u8; 32],
) -> ProgramResult {
    take::Verifier {
        proof,
        public_inputs: take::PublicInputs {
            expiry,
            first_nullifier,
        },
        private_tx_hash,
        transfers: &[],
    }
    .verify()
    .map_err(SwapError::from)?;
    Ok(())
}
