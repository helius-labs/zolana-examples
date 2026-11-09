use anyhow::{ensure, Result};
use solana_address::Address;
use solana_instruction::{AccountMeta, Instruction};
use spr_client::{ProofAuthority, SprIndexer};
use spr_hasher::primitives::solana_owner_identity;
use spr_interface::instruction::instruction_data::transact::TransactIxData;
use spr_keypair::ShieldedKeypair;
use spr_program::{instruction::Transact, Field};
use spr_transaction::instructions::transact::SppProofInputs;
use wincode::{config::DefaultConfig, SchemaWrite};

use crate::{err, escrow_authority_pda};

pub(crate) fn field(bytes: [u8; 32]) -> Result<Field> {
    spr_program::from_bytes(&bytes).map_err(err)
}

/// `hash_bytes` of the creator's tagged Solana pubkey, which the program
/// computes from the creator signer account.
pub(crate) fn owner_pk_field(creator: &ShieldedKeypair) -> Result<Field> {
    let address = creator
        .shielded_address()
        .map_err(err)?
        .solana_address()
        .map_err(err)?;
    field(solana_owner_identity(address.as_array()).map_err(err)?)
}

/// Proves a transaction whose real inputs `authority` holds the nullifier
/// secret of through `indexer`, which serves their Merkle proofs, and checks
/// that the SPP proof covers `private_tx_hash`, the transaction the escrow
/// program proof covers.
pub(crate) fn prove_transact(
    indexer: &SprIndexer,
    spp_proof_inputs: SppProofInputs,
    authority: &dyn ProofAuthority,
) -> Result<TransactIxData> {
    let private_tx_hash = spp_proof_inputs
        .padding_independent_private_tx_hash()
        .map_err(err)?;
    let spp = indexer
        .prove_transact(spp_proof_inputs, authority)
        .map_err(err)?;
    ensure!(
        spp.private_tx_hash == private_tx_hash,
        "the SPP proof and the escrow proof cover different transactions"
    );
    Ok(spp)
}

/// An escrow program instruction: `program_accounts`, then the SPP
/// `transact` account list with `owner_signers`, then the escrow-authority PDA
/// as a non-signer (the program signs for it inside the CPI).
pub(crate) fn escrow_instruction<T>(
    tag: u8,
    program_accounts: Vec<AccountMeta>,
    owner_signers: Vec<Address>,
    payer: Address,
    tree: Address,
    spp: &TransactIxData,
    ix_data: &T,
) -> Result<Instruction>
where
    T: SchemaWrite<DefaultConfig, Src = T>,
{
    let transact = Transact {
        payer,
        input_trees: vec![tree; spp.tree_contexts.len()],
        output_tree: tree,
        owner_signers,
        interface_transfer_accounts: Vec::new(),
        data: spp.clone(),
    };
    let mut accounts = program_accounts;
    accounts.extend(transact.cpi_accounts(&[escrow_authority_pda()]));

    let mut data = vec![tag];
    data.extend_from_slice(&wincode::serialize(ix_data).map_err(err)?);
    Ok(Instruction {
        program_id: timelock_escrow_program::ID,
        accounts,
        data,
    })
}
