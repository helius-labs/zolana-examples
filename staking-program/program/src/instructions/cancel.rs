use pinocchio::{
    cpi::{Seed, Signer},
    error::ProgramError,
    AccountView, Address, ProgramResult,
};
use wincode::{SchemaRead, SchemaWrite};
use zolana_account_checks::AccountIterator;
use zolana_hasher::{
    primitives::{hash_bytes, right_align, solana_owner_identity},
    Hasher, Poseidon,
};
use zolana_interface::instruction::instruction_data::transact::TransactIxData;
use zolana_program::cpi::SppTransactAccounts;

use crate::{
    error::StakingError,
    instructions::proof::{self, LogicProof},
    pda,
    state::{self, Config},
};

pub const REFUND_CIPHERTEXT_BYTES: usize = 301;

#[derive(Clone, Debug, PartialEq, Eq, SchemaRead, SchemaWrite)]
pub struct CancelIntentIxData {
    pub cancel_proof: LogicProof,
    pub spp: TransactIxData,
}

#[derive(Clone, Copy, Debug)]
pub struct CancelProofContext {
    pub private_tx_hash: [u8; 32],
    pub nullifier_hash: [u8; 32],
    pub escrow_owner_identity: [u8; 32],
    pub signer_identity: [u8; 32],
    pub epoch_hash: [u8; 32],
    pub pool_mint_hash: [u8; 32],
    pub crank_viewing_hash: [u8; 32],
    pub input_tree_id: u16,
    pub output_tree_id: u16,
    pub ciphertext_context_hash: [u8; 32],
}

impl CancelProofContext {
    pub fn hash(&self) -> Result<[u8; 32], ProgramError> {
        Poseidon::hashv(&[
            &self.private_tx_hash,
            &self.nullifier_hash,
            &self.escrow_owner_identity,
            &self.signer_identity,
            &self.epoch_hash,
            &self.pool_mint_hash,
            &self.crank_viewing_hash,
            &right_align(&self.input_tree_id.to_be_bytes()),
            &right_align(&self.output_tree_id.to_be_bytes()),
            &self.ciphertext_context_hash,
        ])
        .map_err(|_| StakingError::HashingFailed.into())
    }
}

pub fn proof_context(
    config_key: &Address,
    config: &Config,
    epoch: &Address,
    signer: &Address,
    input_tree_id: u16,
    output_tree_id: u16,
    spp: &TransactIxData,
) -> Result<CancelProofContext, ProgramError> {
    if spp.circuit.num_inputs() != 2
        || spp.circuit.num_outputs() != 2
        || spp.circuit.cache_access().is_some()
        || spp.inputs.len() != 1
        || spp.outputs.len() != 1
        || spp.tree_contexts.len() != 1
        || !spp.interface_transfers.is_empty()
        || spp.data_hash.is_some()
        || spp.ring_data_hash.is_some()
        || !spp.messages.is_empty()
    {
        return Err(StakingError::InvalidInstructionData.into());
    }
    let input = spp
        .inputs
        .first()
        .ok_or(StakingError::InvalidInstructionData)?;
    if input.tree_index != 0 {
        return Err(StakingError::InvalidTree.into());
    }
    let output = spp.outputs.first().ok_or(StakingError::InvalidOutput)?;
    if output.owner_tag
        != zolana_interface::instruction::instruction_data::transact::OwnerTag::Inline(
            signer.to_bytes(),
        )
    {
        return Err(StakingError::InvalidRecipient.into());
    }
    let ciphertext = output
        .data
        .as_deref()
        .ok_or(StakingError::InvalidCiphertext)?;
    let error = |_| StakingError::HashingFailed;
    Ok(CancelProofContext {
        private_tx_hash: spp.private_tx_hash,
        nullifier_hash: input.nullifier_hash,
        escrow_owner_identity: solana_owner_identity(
            pda::escrow_authority(config_key).0.as_array(),
        )
        .map_err(error)?,
        signer_identity: solana_owner_identity(signer.as_array()).map_err(error)?,
        epoch_hash: hash_bytes(epoch.as_array()).map_err(error)?,
        pool_mint_hash: hash_bytes(config.pool_mint.as_array()).map_err(error)?,
        crank_viewing_hash: hash_bytes(&config.crank.viewing_pubkey).map_err(error)?,
        input_tree_id,
        output_tree_id,
        ciphertext_context_hash: proof::ciphertext_context_hash::<REFUND_CIPHERTEXT_BYTES>(
            ciphertext,
            &spp.tx_viewing_pk,
            &spp.salt,
        )?,
    })
}

pub fn process_cancel_intent(accounts: &mut [AccountView], data: &[u8]) -> ProgramResult {
    let ix: CancelIntentIxData =
        wincode::deserialize_exact(data).map_err(|_| StakingError::InvalidInstructionData)?;
    let mut iter = AccountIterator::new(accounts);
    iter.next_signer_mut("payer")?;
    let config_account = iter.next_account("config")?;
    let config = state::load_config(config_account)?;
    let epoch = iter.next_mut("epoch")?;
    let authority_account = iter.next_account("escrow_authority")?;
    let recipient = iter.next_signer("recipient")?;
    let mut header = state::load_epoch(epoch)?;
    if header.config != *config_account.address() {
        return Err(StakingError::InvalidConfig.into());
    }
    if header.locked {
        return Err(StakingError::EpochLocked.into());
    }
    let spp_accounts = iter.remaining()?;
    let (authority, bump) = pda::escrow_authority(config_account.address());
    if authority_account.address() != &authority {
        return Err(StakingError::InvalidSeeds.into());
    }
    let signers = [&authority];
    let spp = SppTransactAccounts::new(spp_accounts, &signers)?;
    let input_tree = spp_accounts
        .get(4)
        .ok_or(ProgramError::NotEnoughAccountKeys)?;
    if input_tree.address() != &config.escrow_tree {
        return Err(StakingError::InvalidTree.into());
    }
    let input_tree_id = zolana_program::compression::load_tree_id(input_tree)
        .map_err(|_| StakingError::InvalidTree)?;
    let output_tree_id = zolana_program::compression::load_tree_id(spp.output_tree()?)
        .map_err(|_| StakingError::InvalidTree)?;
    let context = proof_context(
        config_account.address(),
        &config,
        epoch.address(),
        recipient.address(),
        input_tree_id,
        output_tree_id,
        &ix.spp,
    )?;
    proof::verify(
        &ix.cancel_proof,
        context.hash()?,
        &crate::verifying_keys::cancel::VERIFYINGKEY,
    )?;
    header.resolve(1)?;
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
    state::write_epoch(&mut epoch.try_borrow_mut()?, &header)
}
