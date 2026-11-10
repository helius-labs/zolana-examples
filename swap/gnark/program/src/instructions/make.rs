use borsh::{BorshDeserialize, BorshSerialize};
use light_program_profiler::profile;
use pinocchio::{error::ProgramError, AccountView, ProgramResult};
use spr_account_checks::AccountIterator;
use spr_hasher::{primitives::solana_owner_identity, Hasher, Poseidon};
use spr_interface::instruction::instruction_data::transact::TransactIxData;
use wincode::{SchemaRead, SchemaWrite};

use crate::{
    error::SwapError,
    instructions::{
        shared::OrderAuthority,
        verifier::{verify_groth16, CompressedGroth16Proof},
    },
    verifying_keys::make,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, BorshDeserialize, BorshSerialize)]
pub struct MarkerData {
    pub order_utxo_hash: [u8; 32],
    pub maker_pubkey: [u8; 32],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, SchemaRead, SchemaWrite)]
pub struct MakeProof {
    pub proof_a: [u8; 32],
    pub proof_b: [u8; 64],
    pub proof_c: [u8; 32],
}

#[derive(Clone, Debug, PartialEq, Eq, SchemaRead, SchemaWrite)]
pub struct MakeIxData {
    pub proof: MakeProof,
    pub transact: TransactIxData,
}

const ORDER_OUTPUT_INDEX: usize = 1;

pub struct MakePublicInput<'a> {
    pub private_tx_hash: &'a [u8; 32],
    /// The owner hash of the maker's order-authority PDA, which the circuit
    /// binds the order UTXO's owner to.
    pub order_owner_hash: &'a [u8; 32],
    /// `solana_owner_identity(maker)`: the circuit binds the order terms'
    /// maker owner hash to it, so the terms name the signing maker, and binds
    /// the spent source's owner to that hash, so the source is the maker's own.
    pub maker_owner_pk_field: &'a [u8; 32],
}

impl MakePublicInput<'_> {
    pub fn hash(&self) -> Result<[u8; 32], ProgramError> {
        Poseidon::hashv(&[
            self.private_tx_hash.as_slice(),
            self.order_owner_hash.as_slice(),
            self.maker_owner_pk_field.as_slice(),
        ])
        .map_err(|_| SwapError::HashingFailed.into())
    }
}

/// Verifies the make proof against the Poseidon hash of its public inputs.
#[inline(never)]
#[profile]
pub fn verify_make_proof(proof: &MakeProof, public_input: MakePublicInput) -> ProgramResult {
    verify_groth16(
        CompressedGroth16Proof {
            a: &proof.proof_a,
            b: &proof.proof_b,
            c: &proof.proof_c,
        },
        public_input.hash()?,
        &make::VERIFYINGKEY,
    )
}

#[inline(never)]
#[profile]
pub fn process_make_ix(accounts: &mut [AccountView], data: &[u8]) -> ProgramResult {
    let mut iter = AccountIterator::new(accounts);
    // The maker pays and signs. It signs the SPP transact as the owner of
    // the source it spends; its order-authority PDA, which the program signs
    // for, owns the order UTXO.
    let maker = *iter.next_signer_mut("payer")?.address();
    let order_authority = OrderAuthority::find(&maker);
    let order_owner_hash = order_authority.owner_hash()?;
    let maker_owner_pk_field = solana_owner_identity(maker.as_array()).map_err(SwapError::from)?;

    let MakeIxData {
        proof,
        mut transact,
    } = wincode::deserialize_exact(data).map_err(|_| SwapError::InvalidInstructionData)?;

    verify_make_proof(
        &proof,
        MakePublicInput {
            private_tx_hash: &transact.private_tx_hash,
            order_owner_hash: &order_owner_hash,
            maker_owner_pk_field: &maker_owner_pk_field,
        },
    )?;
    let order_utxo_hash = transact
        .outputs
        .get(ORDER_OUTPUT_INDEX)
        .ok_or(SwapError::InvalidInstructionData)?
        .utxo_hash;
    let [marker_message] = transact.messages.as_mut_slice() else {
        return Err(SwapError::InvalidMarkerMessage.into());
    };
    if !marker_message.data.is_empty() {
        return Err(SwapError::MarkerDataNotEmpty.into());
    }
    let marker = MarkerData {
        order_utxo_hash,
        maker_pubkey: *maker.as_array(),
    };
    marker_message.data = borsh::to_vec(&marker).map_err(|_| SwapError::InvalidInstructionData)?;
    let transact_bytes = transact
        .serialize()
        .map_err(|_| SwapError::InvalidInstructionData)?;

    // The owner signers are the maker, a transaction signer, and its
    // order-authority PDA, which SPP requires to sign for the data-bearing
    // order output.
    let spp_accounts = iter.remaining()?;
    order_authority.cpi_spp_transact_signed(spp_accounts, &transact_bytes)
}
