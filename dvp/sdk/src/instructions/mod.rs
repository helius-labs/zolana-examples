pub mod abort_dvp;
pub mod cancel_dvp;
pub mod create_dvp;
pub mod fund_dvp;
pub mod reclaim_dvp;
pub mod reject_dvp;
pub mod settle_dvp;

use anyhow::{ensure, Result};
use solana_address::Address;
use solana_instruction::{AccountMeta, Instruction};
use wincode::{config::DefaultConfig, SchemaWrite};
use zolana_interface::instruction::instruction_data::transact::TransactIxData;
use zolana_program::instruction::Transact;

use crate::err;

/// The accounts every DvP instruction takes besides its SPP list.
struct DvpAccounts {
    payer: Address,
    /// Signer checked by the instruction proof.
    signer: Address,
    /// The SPP tree: output tree, and input tree of every tree context.
    tree: Address,
    /// With `dvp_id` and `bump` it seeds `dvp_authority`.
    creator: Address,
    dvp_id: [u8; 32],
    bump: u8,
}

/// 1. Check the DvP authority bump.
/// 2. Forward the SDK's SPP account list.
/// 3. Encode the DvP instruction.
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
    // 1. Check the DvP authority bump.
    let (dvp_authority, canonical_bump) = Address::try_find_program_address(
        &[
            dvp_program::DVP_AUTHORITY_PDA_SEED,
            creator.as_ref(),
            &dvp_id,
        ],
        &dvp_program::ID,
    )
    .ok_or_else(|| anyhow::anyhow!("cannot derive the DvP authority"))?;
    ensure!(
        bump == canonical_bump,
        "the DvP authority bump is not canonical"
    );
    // 2. Forward the SDK's SPP account list.
    let mut forwarded = Transact {
        payer,
        input_trees: vec![tree; spp.tree_contexts.len()],
        output_tree: tree,
        owner_signers: vec![dvp_authority],
        interface_transfer_accounts: Vec::new(),
        data: spp.clone(),
    }
    .instruction()
    .accounts;
    for account in &mut forwarded {
        if account.pubkey == dvp_authority {
            account.is_signer = false;
        }
    }
    let mut metas = vec![
        AccountMeta::new(payer, true),
        AccountMeta::new_readonly(signer, true),
    ];
    metas.extend(forwarded);

    // 3. Encode the DvP instruction.
    let mut data = vec![tag];
    data.extend_from_slice(&wincode::serialize(ix_data).map_err(err)?);
    Ok(Instruction {
        program_id: dvp_program::ID,
        accounts: metas,
        data,
    })
}
