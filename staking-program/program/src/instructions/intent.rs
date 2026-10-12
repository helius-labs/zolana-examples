use pinocchio::{
    cpi::{Seed, Signer},
    error::ProgramError,
    sysvars::{clock::Clock, Sysvar},
    AccountView, Address, ProgramResult,
};
use wincode::{SchemaRead, SchemaWrite};
use zolana_account_checks::AccountIterator;
use zolana_hasher::{
    primitives::{hash_bytes, right_align, solana_owner_identity},
    Hasher, Poseidon,
};
use zolana_interface::instruction::instruction_data::transact::{OwnerTag, TransactIxData};
use zolana_program::cpi::SppTransactAccounts;

use crate::{
    error::StakingError,
    instructions::proof::{self, LogicProof},
    pda,
    state::{self, Config},
};

pub const INTENT_CIPHERTEXT_BYTES: usize = 563;

#[derive(Clone, Debug, PartialEq, Eq, SchemaRead, SchemaWrite)]
pub struct CreateIntentIxData {
    pub intent_proof: LogicProof,
    pub spp: TransactIxData,
}

#[derive(Clone, Copy, Debug)]
pub struct IntentProofContext {
    pub private_tx_hash: [u8; 32],
    pub escrow_hash: [u8; 32],
    pub escrow_owner_hash: [u8; 32],
    pub escrow_owner_identity: [u8; 32],
    pub epoch_hash: [u8; 32],
    pub pool_mint_hash: [u8; 32],
    pub crank_viewing_hash: [u8; 32],
    pub tree_id: u16,
    pub input_count: u8,
    pub output_count: u8,
    pub ciphertext_context_hash: [u8; 32],
}

impl IntentProofContext {
    pub fn hash(&self) -> Result<[u8; 32], ProgramError> {
        Poseidon::hashv(&[
            &self.private_tx_hash,
            &self.escrow_hash,
            &self.escrow_owner_hash,
            &self.escrow_owner_identity,
            &self.epoch_hash,
            &self.pool_mint_hash,
            &self.crank_viewing_hash,
            &right_align(&self.tree_id.to_be_bytes()),
            &right_align(&[self.input_count]),
            &right_align(&[self.output_count]),
            &self.ciphertext_context_hash,
        ])
        .map_err(|_| StakingError::HashingFailed.into())
    }
}

pub fn proof_context(
    config_key: &Address,
    config: &Config,
    epoch: &Address,
    tree_id: u16,
    spp: &TransactIxData,
) -> Result<IntentProofContext, ProgramError> {
    if spp.circuit.num_inputs() != 2
        || spp.circuit.num_outputs() != 2
        || spp.circuit.cache_access().is_some()
        || !(1..=2).contains(&spp.inputs.len())
        || !(1..=2).contains(&spp.outputs.len())
        || !spp.interface_transfers.is_empty()
        || spp.data_hash.is_some()
        || spp.ring_data_hash.is_some()
        || !spp.messages.is_empty()
    {
        return Err(StakingError::InvalidInstructionData.into());
    }
    let authority = pda::escrow_authority(config_key).0;
    let output = spp.outputs.first().ok_or(StakingError::InvalidOutput)?;
    if output.owner_tag != OwnerTag::Inline(authority.to_bytes()) {
        return Err(StakingError::InvalidOutput.into());
    }
    let ciphertext: &[u8; INTENT_CIPHERTEXT_BYTES] = output
        .data
        .as_deref()
        .ok_or(StakingError::InvalidCiphertext)?
        .try_into()
        .map_err(|_| StakingError::InvalidCiphertext)?;
    if ciphertext[0] != 1 {
        return Err(StakingError::InvalidCiphertext.into());
    }
    let hashing = |_| StakingError::HashingFailed;
    let identity = solana_owner_identity(authority.as_array()).map_err(hashing)?;
    let zero_pk = Poseidon::hashv(&[&[0; 32]]).map_err(hashing)?;
    let owner = Poseidon::hashv(&[&identity, &zero_pk]).map_err(hashing)?;
    let ciphertext_context_hash = Poseidon::hashv(&[
        &hash_bytes(ciphertext).map_err(hashing)?,
        &hash_bytes(&spp.tx_viewing_pk).map_err(hashing)?,
        &right_align(&spp.salt),
    ])
    .map_err(hashing)?;
    Ok(IntentProofContext {
        private_tx_hash: spp.private_tx_hash,
        escrow_hash: output.utxo_hash,
        escrow_owner_hash: owner,
        escrow_owner_identity: identity,
        epoch_hash: hash_bytes(epoch.as_array()).map_err(hashing)?,
        pool_mint_hash: hash_bytes(config.pool_mint.as_array()).map_err(hashing)?,
        crank_viewing_hash: hash_bytes(&config.crank.viewing_pubkey).map_err(hashing)?,
        tree_id,
        input_count: spp.inputs.len() as u8,
        output_count: spp.outputs.len() as u8,
        ciphertext_context_hash,
    })
}

pub fn process_create_intent(accounts: &mut [AccountView], data: &[u8]) -> ProgramResult {
    let ix: CreateIntentIxData =
        wincode::deserialize_exact(data).map_err(|_| StakingError::InvalidInstructionData)?;
    let mut iter = AccountIterator::new(accounts);
    let payer = iter.next_signer_mut("payer")?;
    let config_account = iter.next_account("config")?;
    let config = state::load_config(config_account)?;
    let epoch = iter.next_mut("epoch")?;
    let authority_account = iter.next_account("escrow_authority")?;
    if iter.next_account("system_program")?.address() != &pinocchio_system::ID {
        return Err(StakingError::InvalidAccount.into());
    }
    let header = state::load_epoch(epoch)?;
    if header.config != *config_account.address() {
        return Err(StakingError::InvalidConfig.into());
    }
    if header.locked {
        return Err(StakingError::EpochLocked.into());
    }
    let clock = Clock::get()?;
    if clock.epoch != header.epoch_id || clock.slot >= header.submission_end_slot {
        return Err(StakingError::SubmissionsClosed.into());
    }
    // Reserve a chain before paying for proof verification and CPI. Transaction
    // rollback preserves the chain if either proof, SPP, or the fee transfer fails.
    let spp_accounts = iter.remaining()?;
    let (authority, bump) = pda::escrow_authority(config_account.address());
    if authority_account.address() != &authority {
        return Err(StakingError::InvalidSeeds.into());
    }
    let signer_pdas = [&authority];
    let spp = SppTransactAccounts::new(spp_accounts, &signer_pdas)?;
    let output_tree = spp.output_tree()?;
    if output_tree.address() != &config.escrow_tree {
        return Err(StakingError::InvalidTree.into());
    }
    let tree_id = zolana_program::compression::load_tree_id(output_tree)
        .map_err(|_| StakingError::InvalidTree)?;
    let context = proof_context(
        config_account.address(),
        &config,
        epoch.address(),
        tree_id,
        &ix.spp,
    )?;
    state::append_intent(
        &mut epoch.try_borrow_mut()?,
        context.escrow_hash,
        config.intent_fee_lamports,
    )?;
    proof::verify(
        &ix.intent_proof,
        context.hash()?,
        &crate::verifying_keys::intent::VERIFYINGKEY,
    )?;
    let bump_bytes = [bump];
    let seeds = [
        Seed::from(pda::ESCROW_AUTHORITY_SEED),
        Seed::from(config_account.address().as_ref()),
        Seed::from(&bump_bytes),
    ];
    spp.invoke::<64>(
        &ix.spp
            .serialize()
            .map_err(|_| StakingError::InvalidInstructionData)?,
        &[Signer::from(&seeds)],
    )?;
    if config.intent_fee_lamports != 0 {
        pinocchio_system::instructions::Transfer {
            from: payer,
            to: epoch,
            lamports: config.intent_fee_lamports,
        }
        .invoke()?;
    }
    Ok(())
}
