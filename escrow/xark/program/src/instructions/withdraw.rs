use light_program_profiler::profile;
use pinocchio::{
    sysvars::{clock::Clock, Sysvar},
    AccountView, ProgramResult,
};
use spr_account_checks::AccountIterator;
use spr_hasher::primitives::solana_owner_identity;
use spr_interface::instruction::instruction_data::transact::TransactIxData;
use wincode::{SchemaRead, SchemaWrite};

use crate::{
    error::TimelockEscrowError,
    instructions::shared::{check_after_window, cpi_spp_transact_signed},
    verifier::withdraw,
};

/// The withdraw proof: compressed Groth16 A, B and C (128 bytes).
pub use spr_program::CompressedProof as WithdrawProof;

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

/// 1. Check the caller and the creator sign.
/// 2. Decode the instruction.
/// 3. Check the escrow has unlocked.
/// 4. Verify the withdraw proof.
/// 5. Invoke SPP with the escrow-authority PDA signer.
#[inline(never)]
#[profile]
pub fn process_withdraw_ix(accounts: &mut [AccountView], data: &[u8]) -> ProgramResult {
    // 1. Check the caller and the creator sign. The withdraw proof recomputes
    // the escrow's committed owner_hash from this pubkey (owner_pk_field), so
    // only the creator can withdraw and the creator knows the refund blinding
    // it chose. The identity is tagged as a Solana key, like the owner hash
    // the escrow terms commit.
    let mut iter = AccountIterator::new(accounts);
    iter.next_signer_mut("caller")?;
    let owner_pk_field = solana_owner_identity(iter.next_signer("creator")?.address().as_array())
        .map_err(TimelockEscrowError::from)?;

    // 2. Decode the instruction.
    let WithdrawIxData {
        proof,
        unlock_timestamp,
        transact,
    } = wincode::deserialize_exact(data)
        .map_err(|_| TimelockEscrowError::InvalidInstructionData)?;

    // 3. Check the escrow has unlocked.
    let clock = Clock::get()?;
    check_after_window(clock.unix_timestamp, unlock_timestamp)?;

    // 4. Verify the withdraw proof.
    withdraw::Verifier {
        proof: &proof,
        public_inputs: withdraw::PublicInputs {
            unlock: unlock_timestamp,
            owner_pk_field,
        },
        private_tx_hash: &transact.private_tx_hash,
        transfers: &[],
    }
    .verify()
    .map_err(TimelockEscrowError::from)?;

    // 5. Invoke SPP with the escrow-authority PDA signer.
    let transact_bytes = transact
        .serialize()
        .map_err(|_| TimelockEscrowError::InvalidInstructionData)?;
    let spp_accounts = iter.remaining()?;
    cpi_spp_transact_signed(spp_accounts, &transact_bytes)
}
