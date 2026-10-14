pub mod abort_dvp;
pub mod cancel_dvp;
pub mod create_dvp;
pub mod fund_dvp;
pub mod reclaim_dvp;
pub mod reject_dvp;
pub mod settle_dvp;

use anyhow::Result;
use dvp_program::instructions::shared::dvp_authority;
use solana_address::Address;
use solana_instruction::{AccountMeta, Instruction};
use wincode::{config::DefaultConfig, SchemaWrite};
use zolana_interface::{
    instruction::instruction_data::transact::TransactIxData, SHIELDED_POOL_PROGRAM_ID,
};
use zolana_program::instruction::nullifier_pda_accounts;

use crate::err;

/// The accounts every DvP instruction takes besides its SPP list.
struct DvpAccounts {
    payer: Address,
    /// The instruction's readonly signer, whose identity its proof binds
    /// (the creator, the settlement authority or a party).
    signer: Address,
    /// The SPP tree: output tree, and input tree of every tree context.
    tree: Address,
    /// With `dvp_id` and `bump` it seeds `dvp_authority`.
    creator: Address,
    dvp_id: [u8; 32],
    bump: u8,
}

/// A DvP instruction: `tag` and the serialized `ix_data`, whose SPP transact
/// is `spp`. The payer and the signer come first; everything after them is
/// the forwarded SPP transact list, with a nullifier PDA for each spent
/// input, ending in the `dvp_authority` owner signer the program signs for.
fn dvp_instruction<T>(
    tag: u8,
    accounts: DvpAccounts,
    spp: &TransactIxData,
    ix_data: &T,
) -> Result<Instruction>
where
    T: SchemaWrite<DefaultConfig, Src = T>,
{
    let DvpAccounts {
        payer,
        signer,
        tree,
        creator,
        dvp_id,
        bump,
    } = accounts;
    let dvp_authority = dvp_authority(&creator, &dvp_id, bump).map_err(err)?;
    let nullifier_pdas =
        nullifier_pda_accounts(&tree, spp.inputs.iter().map(|input| &input.nullifier_hash));
    let input_trees = std::iter::repeat_n(AccountMeta::new(tree, false), spp.tree_contexts.len());

    let mut metas = vec![
        AccountMeta::new(payer, true),
        AccountMeta::new_readonly(signer, true),
        AccountMeta::new(payer, true),
        AccountMeta::new(tree, false),
        AccountMeta::new_readonly(Address::new_from_array(SHIELDED_POOL_PROGRAM_ID), false),
        AccountMeta::new_readonly(Address::default(), false),
    ];
    metas.extend(input_trees);
    metas.extend(nullifier_pdas);
    metas.push(AccountMeta::new_readonly(dvp_authority, false));

    let mut data = vec![tag];
    data.extend_from_slice(&wincode::serialize(ix_data).map_err(err)?);
    Ok(Instruction {
        program_id: dvp_program::ID,
        accounts: metas,
        data,
    })
}
