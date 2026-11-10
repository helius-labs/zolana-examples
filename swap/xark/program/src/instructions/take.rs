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
    instructions::shared::{check_within_window, IxData, OrderAuthority},
    verifier::take,
};

pub use spr_program::CompressedProof as TakeProof;

/// The take arguments the program reads ahead of the transact bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, SchemaRead, SchemaWrite)]
pub struct TakeArgs {
    pub proof: TakeProof,
    /// See [`TakeIxData::maker`].
    pub maker: [u8; 32],
}

/// The take instruction data as a client writes it: [`TakeArgs`], then the
/// transact bytes, which the program reads in place ([`IxData`]) and forwards
/// to SPP unchanged.
#[derive(Clone, Debug, PartialEq, Eq, SchemaRead, SchemaWrite)]
pub struct TakeIxData {
    pub proof: TakeProof,
    /// The order's maker (the marker's `maker_pubkey`), whose order-authority
    /// PDA the program signs for. A wrong maker signs for a PDA that does not
    /// own the order UTXO, and SPP rejects the spend.
    pub maker: [u8; 32],
    pub transact: TransactIxData,
}

/// 1. Check the fee payer.
/// 2. Decode the instruction.
/// 3. Check `now <= expiry`, the expiry being the transact's `expiry_unix_ts`.
/// 4. Verify the take proof against the expiry and the first nullifier.
/// 5. Invoke SPP with the maker's order-authority PDA signer.
#[inline(never)]
#[profile]
pub fn process_take_ix(accounts: &mut [AccountView], data: &[u8]) -> ProgramResult {
    // 1. Check the fee payer.
    let mut iter = AccountIterator::new(accounts);
    iter.next_signer_mut("payer")?;

    // 2. Decode the instruction.
    let IxData {
        args: TakeArgs { proof, maker },
        transact,
        transact_bytes,
    } = IxData::<TakeArgs>::parse(data)?;

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
        transact.private_tx_hash,
    )?;

    // 5. Invoke SPP with the maker's order-authority PDA signer.
    let spp_accounts = iter.remaining()?;
    OrderAuthority::find(&maker).cpi_spp_transact_signed(spp_accounts, transact_bytes)
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
