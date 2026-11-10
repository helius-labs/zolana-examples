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
    error::SwapError,
    instructions::{
        shared::{check_after_window, u64_right_align, IxData, OrderAuthority},
        verifier::{verify_groth16, CompressedGroth16Proof},
    },
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, SchemaRead, SchemaWrite)]
pub struct CancelProof {
    pub proof_a: [u8; 32],
    pub proof_b: [u8; 64],
    pub proof_c: [u8; 32],
}

/// The cancel arguments the program reads ahead of the transact bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, SchemaRead, SchemaWrite)]
pub struct CancelArgs {
    pub proof: CancelProof,
    /// See [`CancelIxData::order_expiry`].
    pub order_expiry: u64,
    /// See [`CancelIxData::authority_bump`].
    pub authority_bump: u8,
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
    /// The bump of the maker's order-authority PDA. The SDK passes the
    /// canonical bump; see [`OrderAuthority::new`].
    pub authority_bump: u8,
    pub transact: TransactIxData,
}

pub struct CancelPublicInput<'a> {
    pub private_tx_hash: &'a [u8; 32],
    pub expiry: u64,
    pub maker_owner_pk_field: &'a [u8; 32],
}

impl CancelPublicInput<'_> {
    pub fn hash(&self) -> Result<[u8; 32], ProgramError> {
        Poseidon::hashv(&[
            self.private_tx_hash.as_slice(),
            u64_right_align(self.expiry).as_slice(),
            self.maker_owner_pk_field.as_slice(),
        ])
        .map_err(|_| SwapError::HashingFailed.into())
    }
}

/// Verifies the cancel proof against the Poseidon hash of its public inputs.
#[inline(never)]
#[profile]
pub fn verify_cancel_proof(proof: &CancelProof, public_input: CancelPublicInput) -> ProgramResult {
    verify_groth16(
        CompressedGroth16Proof {
            a: &proof.proof_a,
            b: &proof.proof_b,
            c: &proof.proof_c,
        },
        public_input.hash()?,
        &crate::verifying_keys::cancel::VERIFYINGKEY,
    )
}

#[inline(never)]
#[profile]
pub fn process_cancel_ix(accounts: &mut [AccountView], data: &[u8]) -> ProgramResult {
    let mut iter = AccountIterator::new(accounts);
    iter.next_signer_mut("payer")?;
    // The maker signs the cancel; the cancel proof recomputes the order's
    // committed maker_owner_hash from this pubkey (owner_pk_field), so only the
    // maker can cancel and the maker knows the refund blinding it chose. The
    // identity is tagged as a Solana key, like the owner hash the order commits.
    let maker = *iter.next_signer("maker")?.address();
    let maker_owner_pk_field = solana_owner_identity(maker.as_array()).map_err(SwapError::from)?;

    // The program reads the transact data in place and forwards the client's
    // bytes to SPP unchanged.
    let IxData {
        args:
            CancelArgs {
                proof,
                order_expiry,
                authority_bump,
            },
        transact,
        transact_bytes,
    } = IxData::<CancelArgs>::parse(data)?;
    let order_authority = OrderAuthority::new(&maker, authority_bump)?;

    let clock = Clock::get()?;
    check_after_window(clock.unix_timestamp, order_expiry)?;

    verify_cancel_proof(
        &proof,
        CancelPublicInput {
            private_tx_hash: transact.private_tx_hash,
            expiry: order_expiry,
            maker_owner_pk_field: &maker_owner_pk_field,
        },
    )?;

    let spp_accounts = iter.remaining()?;
    order_authority.cpi_spp_transact_signed(spp_accounts, transact_bytes)
}
