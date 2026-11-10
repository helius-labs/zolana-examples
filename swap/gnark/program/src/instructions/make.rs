use borsh::{BorshDeserialize, BorshSerialize};
use light_program_profiler::profile;
use pinocchio::{error::ProgramError, AccountView, ProgramResult};
use spr_account_checks::AccountIterator;
use spr_hasher::{primitives::solana_owner_identity, Hasher, Poseidon};
use spr_interface::instruction::instruction_data::transact::{TransactIxData, TransactIxDataRef};
use wincode::{SchemaRead, SchemaWrite};

use crate::{
    error::SwapError,
    instructions::{
        shared::{IxData, OrderAuthority},
        verifier::{verify_groth16, CompressedGroth16Proof},
    },
    verifying_keys::make,
};

/// The make transaction's single message: the order UTXO's hash and the
/// maker, Borsh-encoded (64 bytes), which the taker's discovery reads.
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

/// The make arguments the program reads ahead of the transact bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, SchemaRead, SchemaWrite)]
pub struct MakeArgs {
    pub proof: MakeProof,
    /// See [`MakeIxData::authority_bump`].
    pub authority_bump: u8,
}

/// The make instruction data as a client writes it: [`MakeArgs`], then the
/// transact data, which the program reads in place ([`IxData`]) and forwards
/// to SPP unchanged.
#[derive(Clone, Debug, PartialEq, Eq, SchemaRead, SchemaWrite)]
pub struct MakeIxData {
    pub proof: MakeProof,
    /// The bump of the maker's order-authority PDA. The SDK passes the
    /// canonical bump; see [`OrderAuthority::new`].
    pub authority_bump: u8,
    /// Carries exactly one message, the marker, whose data is the
    /// [`MarkerData`] of the order output and the maker signer.
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
    let maker_owner_pk_field = solana_owner_identity(maker.as_array()).map_err(SwapError::from)?;

    // The program reads the transact data in place and forwards the client's
    // bytes to SPP unchanged.
    let IxData {
        args: MakeArgs {
            proof,
            authority_bump,
        },
        transact,
        transact_bytes,
    } = IxData::<MakeArgs>::parse(data)?;
    let order_authority = OrderAuthority::new(&maker, authority_bump)?;
    let order_owner_hash = order_authority.owner_hash()?;

    // SPP hashes the messages into the external data its proof covers, so the
    // client writes the marker and the program checks it.
    check_marker(&transact, maker.as_array())?;

    verify_make_proof(
        &proof,
        MakePublicInput {
            private_tx_hash: transact.private_tx_hash,
            order_owner_hash: &order_owner_hash,
            maker_owner_pk_field: &maker_owner_pk_field,
        },
    )?;

    // The owner signers are the maker, a transaction signer, and its
    // order-authority PDA, which SPP requires to sign for the data-bearing
    // order output.
    let spp_accounts = iter.remaining()?;
    order_authority.cpi_spp_transact_signed(spp_accounts, transact_bytes)
}

/// The transaction carries exactly one message, whose data is the
/// [`MarkerData`] of the order output's hash and `maker_pubkey`.
fn check_marker(transact: &TransactIxDataRef, maker_pubkey: &[u8; 32]) -> ProgramResult {
    let order_utxo_hash = transact
        .outputs
        .get(ORDER_OUTPUT_INDEX)
        .ok_or(SwapError::InvalidInstructionData)?
        .utxo_hash;
    let [marker_message] = transact.messages.as_slice() else {
        return Err(SwapError::InvalidMarkerMessage.into());
    };
    // Compare the Borsh encoding, `order_utxo_hash || maker_pubkey`, in
    // place. Slice equality also checks the lengths, so the data is 64 bytes.
    let (marker_order_utxo_hash, marker_maker_pubkey) = marker_message
        .data
        .split_at_checked(32)
        .ok_or(SwapError::InvalidMarkerData)?;
    if marker_order_utxo_hash != order_utxo_hash.as_slice()
        || marker_maker_pubkey != maker_pubkey.as_slice()
    {
        return Err(SwapError::InvalidMarkerData.into());
    }
    Ok(())
}
