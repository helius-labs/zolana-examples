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
    instructions::shared::{check_after_window, EscrowAuthority, IxData},
    verifier::withdraw,
};

/// The withdraw proof: compressed Groth16 A, B and C (128 bytes).
pub use spr_program::CompressedProof as WithdrawProof;

/// The withdraw arguments the program reads ahead of the transact bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, SchemaRead, SchemaWrite)]
pub struct WithdrawArgs {
    pub proof: WithdrawProof,
    /// See [`WithdrawIxData::unlock_timestamp`].
    pub unlock_timestamp: u64,
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
    pub transact: TransactIxData,
}

/// 1. Check the caller and the creator sign.
/// 2. Decode the instruction.
/// 3. Check the escrow has unlocked.
/// 4. Verify the withdraw proof.
/// 5. Invoke SPP with the creator's escrow-authority PDA signer.
#[inline(never)]
#[profile]
pub fn process_withdraw_ix(accounts: &mut [AccountView], data: &[u8]) -> ProgramResult {
    // 1. Check the caller and the creator sign. The withdraw proof checks
    // this pubkey (owner_pk_field) against the creator the escrow terms
    // commit, so only the creator can withdraw. The identity is tagged as a
    // Solana key, like the creator's owner key.
    let mut iter = AccountIterator::new(accounts);
    iter.next_signer_mut("caller")?;
    let creator = *iter.next_signer("creator")?.address();
    let owner_pk_field =
        solana_owner_identity(creator.as_array()).map_err(TimelockEscrowError::from)?;

    // 2. Decode the instruction.
    let IxData {
        args: WithdrawArgs {
            proof,
            unlock_timestamp,
        },
        transact,
        transact_bytes,
    } = IxData::<WithdrawArgs>::parse(data)?;

    // 3. Check the escrow has unlocked.
    let clock = Clock::get()?;
    check_after_window(clock.unix_timestamp, unlock_timestamp)?;

    // 4. Verify the withdraw proof.
    verify_withdraw_proof(
        &proof,
        unlock_timestamp,
        owner_pk_field,
        transact.private_tx_hash,
    )?;

    // 5. Invoke SPP with the creator's escrow-authority PDA signer, which
    // the escrow UTXO's owner requires: only the creator's escrows spend.
    let spp_accounts = iter.remaining()?;
    EscrowAuthority::find(&creator).cpi_spp_transact_signed(spp_accounts, transact_bytes)
}

/// Verifies the withdraw proof against the escrow's `unlock` timestamp, the
/// creator's owner identity and the transaction's `private_tx_hash`: the
/// public-input hash, then the Groth16 verification.
#[inline(never)]
#[profile]
pub fn verify_withdraw_proof(
    proof: &WithdrawProof,
    unlock: u64,
    owner_pk_field: [u8; 32],
    private_tx_hash: &[u8; 32],
) -> ProgramResult {
    withdraw::Verifier {
        proof,
        public_inputs: withdraw::PublicInputs {
            unlock,
            owner_pk_field,
        },
        private_tx_hash,
        transfers: &[],
    }
    .verify()
    .map_err(TimelockEscrowError::from)?;
    Ok(())
}
