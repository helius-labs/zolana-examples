use light_program_profiler::profile;
use pinocchio::{
    error::ProgramError,
    sysvars::{clock::Clock, Sysvar},
    AccountView, Address, ProgramResult,
};
use spr_account_checks::AccountIterator;
use spr_hasher::{Hasher, Poseidon};
use spr_interface::instruction::instruction_data::transact::TransactIxData;
use wincode::{SchemaRead, SchemaWrite};

use crate::{
    error::SwapError,
    instructions::{
        shared::{check_within_window, u64_right_align, IxData, OrderAuthority},
        verifier::{verify_groth16, CompressedGroth16Proof},
    },
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, SchemaRead, SchemaWrite)]
pub struct TakeProof {
    pub proof_a: [u8; 32],
    pub proof_b: [u8; 64],
    pub proof_c: [u8; 32],
}

/// The take arguments the program reads ahead of the transact bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, SchemaRead, SchemaWrite)]
pub struct TakeArgs {
    pub proof: TakeProof,
    /// See [`TakeIxData::maker`].
    pub maker: [u8; 32],
    /// See [`TakeIxData::authority_bump`].
    pub authority_bump: u8,
}

/// The take instruction data as a client writes it: [`TakeArgs`], then the
/// transact data, which the program reads in place ([`IxData`]) and forwards
/// to SPP unchanged.
#[derive(Clone, Debug, PartialEq, Eq, SchemaRead, SchemaWrite)]
pub struct TakeIxData {
    pub proof: TakeProof,
    /// The order's maker (the marker's `maker_pubkey`). The program signs for
    /// this maker's order-authority PDA; SPP rejects the spend unless that PDA
    /// owns the order input.
    pub maker: [u8; 32],
    /// The bump of the maker's order-authority PDA. The SDK passes the
    /// canonical bump; see [`OrderAuthority::new`].
    pub authority_bump: u8,
    pub transact: TransactIxData,
}

pub struct TakePublicInput<'a> {
    pub private_tx_hash: &'a [u8; 32],
    pub expiry: u64,
    pub first_nullifier: &'a [u8; 32],
}

impl TakePublicInput<'_> {
    pub fn hash(&self) -> Result<[u8; 32], ProgramError> {
        Poseidon::hashv(&[
            self.private_tx_hash.as_slice(),
            u64_right_align(self.expiry).as_slice(),
            self.first_nullifier.as_slice(),
        ])
        .map_err(|_| SwapError::HashingFailed.into())
    }
}

/// Verifies the take proof against the Poseidon hash of its public inputs.
#[inline(never)]
#[profile]
pub fn verify_take_proof(proof: &TakeProof, public_input: TakePublicInput) -> ProgramResult {
    verify_groth16(
        CompressedGroth16Proof {
            a: &proof.proof_a,
            b: &proof.proof_b,
            c: &proof.proof_c,
        },
        public_input.hash()?,
        &crate::verifying_keys::take::VERIFYINGKEY,
    )
}

#[inline(never)]
#[profile]
pub fn process_take_ix(accounts: &mut [AccountView], data: &[u8]) -> ProgramResult {
    let mut iter = AccountIterator::new(accounts);
    iter.next_signer_mut("payer")?;

    // The program reads the transact data in place and forwards the client's
    // bytes to SPP unchanged.
    let IxData {
        args: TakeArgs {
            proof,
            maker,
            authority_bump,
        },
        transact,
        transact_bytes,
    } = IxData::<TakeArgs>::parse(data)?;
    let order_authority = OrderAuthority::new(&Address::new_from_array(maker), authority_bump)?;

    let clock = Clock::get()?;
    check_within_window(clock.unix_timestamp, transact.expiry_unix_ts)?;

    verify_take_proof(
        &proof,
        TakePublicInput {
            private_tx_hash: transact.private_tx_hash,
            expiry: transact.expiry_unix_ts,
            first_nullifier: &transact
                .inputs
                .first()
                .ok_or(SwapError::InvalidInstructionData)?
                .nullifier_hash,
        },
    )?;

    let spp_accounts = iter.remaining()?;
    order_authority.cpi_spp_transact_signed(spp_accounts, transact_bytes)
}
