use light_program_profiler::profile;
use pinocchio::{AccountView, ProgramResult};
use spr_account_checks::AccountIterator;
use spr_hasher::primitives::solana_owner_identity;
use spr_interface::instruction::instruction_data::transact::TransactIxData;
use wincode::{SchemaRead, SchemaWrite};

use crate::{
    error::TimelockEscrowError,
    instructions::shared::{EscrowAuthority, IxData},
    verifier::escrow,
};

/// The escrow proof: compressed Groth16 A, B and C (128 bytes).
pub use spr_program::CompressedProof as EscrowProof;

/// The escrow arguments the program reads ahead of the transact bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, SchemaRead, SchemaWrite)]
pub struct EscrowArgs {
    pub proof: EscrowProof,
    /// See [`EscrowIxData::viewing_pubkey`].
    pub viewing_pubkey: [u8; 33],
    /// See [`EscrowIxData::authority_bump`].
    pub authority_bump: u8,
}

/// The escrow instruction data as a client writes it: [`EscrowArgs`], then
/// the transact data, which the program reads in place ([`IxData`]) and
/// forwards to SPP unchanged.
#[derive(Clone, Debug, PartialEq, Eq, SchemaRead, SchemaWrite)]
pub struct EscrowIxData {
    pub proof: EscrowProof,
    /// The viewing pubkey of the escrow UTXO's address. The program hashes it
    /// with the creator's escrow-authority PDA owner hash into the escrow
    /// proof's `escrow_owner`; it chooses who can decrypt the escrow UTXO,
    /// never who can spend it.
    pub viewing_pubkey: [u8; 33],
    /// The bump of the creator's escrow-authority PDA; see
    /// [`EscrowAuthority::new`].
    pub authority_bump: u8,
    pub transact: TransactIxData,
}

/// 1. Check the creator signs.
/// 2. Decode the instruction and derive the creator's escrow-authority PDA.
/// 3. Verify the escrow proof against the creator, the escrow UTXO's address
///    (the creator's escrow-authority PDA) and the transaction's
///    `private_tx_hash`.
/// 4. Invoke SPP with the escrow-authority PDA signer, which the escrow UTXO's
///    data requires.
#[inline(never)]
#[profile]
pub fn process_escrow_ix(accounts: &mut [AccountView], data: &[u8]) -> ProgramResult {
    // 1. Check the creator signs. The escrow proof checks the source owner
    // against this pubkey (owner_pk_field), tagged as a Solana key like the
    // owner the escrow terms commit. The creator is read-only: the fee payer
    // is the separate SPP payer among the forwarded accounts, and may be the
    // same key.
    let mut iter = AccountIterator::new(accounts);
    let creator = *iter.next_signer("creator")?.address();
    let owner_pk_field =
        solana_owner_identity(creator.as_array()).map_err(TimelockEscrowError::from)?;

    // 2. Decode the instruction and derive the escrow-authority PDA.
    let IxData {
        args:
            EscrowArgs {
                proof,
                viewing_pubkey,
                authority_bump,
            },
        transact,
        transact_bytes,
    } = IxData::<EscrowArgs>::parse(data)?;
    let escrow_authority = EscrowAuthority::new(&creator, authority_bump)?;

    // 3. Verify the escrow proof against the creator, the escrow UTXO's
    // address and the transaction's `private_tx_hash`. The proof outputs the
    // escrow UTXO to this address, so only the program spends it.
    let escrow_owner = escrow_authority.address_hash(&viewing_pubkey)?;
    verify_escrow_proof(
        &proof,
        owner_pk_field,
        escrow_owner,
        transact.private_tx_hash,
    )?;

    // 4. Invoke SPP with the escrow-authority PDA signer, which the escrow
    // UTXO's data requires.
    let spp_accounts = iter.remaining()?;
    escrow_authority.cpi_spp_transact_signed(spp_accounts, transact_bytes)
}

/// Verifies the escrow proof against the creator's owner identity, the escrow
/// UTXO's address hash and the transaction's `private_tx_hash`: the
/// public-input hash, then the Groth16 verification.
#[inline(never)]
#[profile]
pub fn verify_escrow_proof(
    proof: &EscrowProof,
    owner_pk_field: [u8; 32],
    escrow_owner: [u8; 32],
    private_tx_hash: &[u8; 32],
) -> ProgramResult {
    escrow::Verifier {
        proof,
        public_inputs: escrow::PublicInputs {
            owner_pk_field,
            escrow_owner,
        },
        private_tx_hash,
        transfers: &[],
    }
    .verify()
    .map_err(TimelockEscrowError::from)?;
    Ok(())
}
