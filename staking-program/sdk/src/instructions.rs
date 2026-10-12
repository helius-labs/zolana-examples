use anyhow::Result;
use private_staking_program::{
    instructions::intent::CreateIntentIxData,
    instructions::management::{
        CreateBatchIxData, CreateConfigIxData, CreateEpochIxData, FillBatchIxData, ResetBatchIxData,
    },
    pda, tag, ID,
};
use solana_address::Address;
use solana_instruction::{AccountMeta, Instruction};
use wincode::SchemaWrite;

fn instruction<T: SchemaWrite<wincode::config::DefaultConfig, Src = T>>(
    tag: u8,
    accounts: Vec<AccountMeta>,
    data: &T,
) -> Result<Instruction> {
    let mut bytes = vec![tag];
    bytes.extend(wincode::serialize(data)?);
    Ok(Instruction {
        program_id: ID,
        accounts,
        data: bytes,
    })
}

pub fn create_intent(
    payer: Address,
    config: Address,
    epoch: Address,
    data: CreateIntentIxData,
    spp_accounts: &[AccountMeta],
) -> Result<Instruction> {
    let mut accounts = vec![
        AccountMeta::new(payer, true),
        AccountMeta::new_readonly(config, false),
        AccountMeta::new(epoch, false),
        AccountMeta::new_readonly(pda::escrow_authority(&config).0, false),
        AccountMeta::new_readonly(Address::default(), false),
    ];
    let authority = pda::escrow_authority(&config).0;
    accounts.extend(spp_accounts.iter().cloned().map(|mut meta| {
        if meta.pubkey == authority {
            meta.is_signer = false;
        }
        meta
    }));
    instruction(tag::CREATE_INTENT, accounts, &data)
}

pub fn cancel_intent(
    payer: Address,
    recipient: Address,
    config: Address,
    epoch: Address,
    data: private_staking_program::instructions::cancel::CancelIntentIxData,
    spp_accounts: &[AccountMeta],
) -> Result<Instruction> {
    let authority = pda::escrow_authority(&config).0;
    let mut accounts = vec![
        AccountMeta::new(payer, true),
        AccountMeta::new_readonly(config, false),
        AccountMeta::new(epoch, false),
        AccountMeta::new_readonly(authority, false),
        AccountMeta::new_readonly(recipient, true),
    ];
    accounts.extend(spp_accounts.iter().cloned().map(|mut meta| {
        if meta.pubkey == authority {
            meta.is_signer = false;
        }
        meta
    }));
    instruction(tag::CANCEL_INTENT, accounts, &data)
}

fn empty_instruction(tag: u8, accounts: Vec<AccountMeta>) -> Instruction {
    Instruction {
        program_id: ID,
        accounts,
        data: vec![tag],
    }
}

pub fn create_config(
    authority: Address,
    payer: Address,
    pool: Address,
    escrow_tree: Address,
    data: CreateConfigIxData,
) -> Result<Instruction> {
    instruction(
        tag::CREATE_CONFIG,
        vec![
            AccountMeta::new_readonly(authority, true),
            AccountMeta::new(payer, true),
            AccountMeta::new_readonly(pool, false),
            AccountMeta::new_readonly(escrow_tree, false),
            AccountMeta::new(pda::config(&pool).0, false),
            AccountMeta::new_readonly(Address::default(), false),
        ],
        &data,
    )
}

pub fn create_epoch(config: Address, epoch_id: u64, page_index: u64) -> Result<Instruction> {
    let mut accounts = vec![
        AccountMeta::new_readonly(config, false),
        AccountMeta::new(pda::epoch(&config, epoch_id, page_index).0, false),
        AccountMeta::new(pda::rent_payer(&config).0, false),
        AccountMeta::new_readonly(Address::default(), false),
    ];
    if page_index > 0 {
        accounts.push(AccountMeta::new_readonly(
            pda::epoch(&config, epoch_id, page_index - 1).0,
            false,
        ));
    }
    instruction(
        tag::CREATE_EPOCH,
        accounts,
        &CreateEpochIxData { page_index },
    )
}

pub fn lock_epoch(crank: Address, config: Address, epoch: Address) -> Instruction {
    empty_instruction(
        tag::LOCK_EPOCH,
        vec![
            AccountMeta::new_readonly(crank, true),
            AccountMeta::new_readonly(config, false),
            AccountMeta::new(epoch, false),
        ],
    )
}

pub fn unlock_epoch(epoch: Address) -> Instruction {
    empty_instruction(tag::UNLOCK_EPOCH, vec![AccountMeta::new(epoch, false)])
}

pub fn create_batch(
    payer: Address,
    crank: Address,
    config: Address,
    epoch: Address,
    data: CreateBatchIxData,
) -> Result<Instruction> {
    instruction(
        tag::CREATE_BATCH,
        vec![
            AccountMeta::new(payer, true),
            AccountMeta::new_readonly(crank, true),
            AccountMeta::new_readonly(config, false),
            AccountMeta::new(epoch, false),
            AccountMeta::new(pda::batch(&epoch, data.hash_chain_index).0, false),
            AccountMeta::new_readonly(Address::default(), false),
        ],
        &data,
    )
}

pub fn fill_batch(
    crank: Address,
    epoch: Address,
    batch: Address,
    data: FillBatchIxData,
) -> Result<Instruction> {
    instruction(tag::FILL_BATCH, batch_accounts(crank, epoch, batch), &data)
}

pub fn reset_batch(
    crank: Address,
    epoch: Address,
    batch: Address,
    data: ResetBatchIxData,
) -> Result<Instruction> {
    instruction(tag::RESET_BATCH, batch_accounts(crank, epoch, batch), &data)
}

fn batch_accounts(crank: Address, epoch: Address, batch: Address) -> Vec<AccountMeta> {
    vec![
        AccountMeta::new_readonly(crank, true),
        AccountMeta::new_readonly(epoch, false),
        AccountMeta::new(batch, false),
    ]
}

/// Pass unused batches with their recorded rent payers for incremental cleanup.
pub fn close_epoch(
    config: Address,
    epoch: Address,
    crank: Address,
    unused_batches: &[(Address, Address)],
) -> Instruction {
    let mut accounts = vec![
        AccountMeta::new_readonly(config, false),
        AccountMeta::new(epoch, false),
        AccountMeta::new(pda::rent_payer(&config).0, false),
        AccountMeta::new(crank, false),
    ];
    for &(batch, payer) in unused_batches {
        accounts.extend([
            AccountMeta::new(batch, false),
            AccountMeta::new(payer, false),
        ]);
    }
    empty_instruction(tag::CLOSE_EPOCH, accounts)
}
