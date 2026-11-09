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
    error::SwapError,
    instructions::shared::{check_after_window, cpi_spp_transact_signed, IxData},
    verifier::cancel,
};

pub use spr_program::CompressedProof as CancelProof;

/// The cancel arguments the program reads ahead of the transact bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, SchemaRead, SchemaWrite)]
pub struct CancelArgs {
    pub proof: CancelProof,
    /// See [`CancelIxData::order_expiry`].
    pub order_expiry: u64,
}

/// The cancel instruction data as a client writes it: [`CancelArgs`], then
/// the transact data, which the program reads in place ([`IxData`]) and
/// forwards to SPP unchanged.
#[derive(Clone, Debug, PartialEq, Eq, SchemaRead, SchemaWrite)]
pub struct CancelIxData {
    pub proof: CancelProof,
    /// The committed order `expiry` the cancel proof reveals as a public input.
    /// Separate from `transact.expiry_unix_ts` (the SPP relayer deadline): cancel
    /// requires `now > order_expiry`, which is necessarily in the past, whereas
    /// SPP rejects a `transact` whose `expiry_unix_ts` is in the past. The
    /// order's committed terms hash includes `order_expiry`; the proof
    /// recomputes it.
    pub order_expiry: u64,
    pub transact: TransactIxData,
}

/// 1. Check the fee payer and the maker signer.
/// 2. Decode the instruction.
/// 3. Check `now > order_expiry`.
/// 4. Verify the cancel proof against the expiry and the maker.
/// 5. Invoke SPP with the order-authority PDA signer.
#[inline(never)]
#[profile]
pub fn process_cancel_ix(accounts: &mut [AccountView], data: &[u8]) -> ProgramResult {
    // 1. The maker signs the cancel; the cancel proof recomputes the order's
    // committed maker_owner_hash from this pubkey, so only the maker can
    // cancel. The identity is tagged as a Solana key, like the owner hash the
    // order commits.
    let mut iter = AccountIterator::new(accounts);
    iter.next_signer_mut("payer")?;
    let maker_owner_pk_field =
        solana_owner_identity(iter.next_signer("maker")?.address().as_array())
            .map_err(SwapError::from)?;

    // 2. Decode the instruction.
    let IxData {
        args: CancelArgs {
            proof,
            order_expiry,
        },
        transact,
        transact_bytes,
    } = IxData::<CancelArgs>::parse(data)?;

    // 3. Check `now > order_expiry`.
    let clock = Clock::get()?;
    check_after_window(clock.unix_timestamp, order_expiry)?;

    // 4. Verify the cancel proof.
    verify_cancel_proof(
        &proof,
        order_expiry,
        maker_owner_pk_field,
        transact.private_tx_hash,
    )?;

    // 5. Invoke SPP with the order-authority PDA signer.
    let spp_accounts = iter.remaining()?;
    cpi_spp_transact_signed(spp_accounts, transact_bytes)
}

/// Hashes the cancel proof's public inputs and verifies the proof.
#[inline(never)]
#[profile]
fn verify_cancel_proof(
    proof: &CancelProof,
    expiry: u64,
    maker_owner_pk_field: [u8; 32],
    private_tx_hash: &[u8; 32],
) -> ProgramResult {
    cancel::Verifier {
        proof,
        public_inputs: cancel::PublicInputs {
            expiry,
            maker_owner_pk_field,
        },
        private_tx_hash,
        transfers: &[],
    }
    .verify()
    .map_err(SwapError::from)?;
    Ok(())
}
