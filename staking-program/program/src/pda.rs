use pinocchio::Address;

pub const CONFIG_SEED: &[u8] = b"config";
pub const EPOCH_SEED: &[u8] = b"epoch";
pub const BATCH_SEED: &[u8] = b"batch";
pub const RENT_PAYER_SEED: &[u8] = b"rent_payer";
pub const ESCROW_AUTHORITY_SEED: &[u8] = b"escrow_authority";
pub const SOL_VAULT_SEED: &[u8] = b"sol_vault";

pub fn config(pool: &Address) -> (Address, u8) {
    Address::find_program_address(&[CONFIG_SEED, pool.as_ref()], &crate::ID)
}

pub fn epoch(config: &Address, epoch_id: u64, page_index: u64) -> (Address, u8) {
    Address::find_program_address(
        &[
            EPOCH_SEED,
            config.as_ref(),
            &epoch_id.to_le_bytes(),
            &page_index.to_le_bytes(),
        ],
        &crate::ID,
    )
}

pub fn batch(epoch: &Address, index: u16) -> (Address, u8) {
    Address::find_program_address(
        &[BATCH_SEED, epoch.as_ref(), &index.to_le_bytes()],
        &crate::ID,
    )
}

pub fn rent_payer(config: &Address) -> (Address, u8) {
    Address::find_program_address(&[RENT_PAYER_SEED, config.as_ref()], &crate::ID)
}

pub fn escrow_authority(config: &Address) -> (Address, u8) {
    Address::find_program_address(&[ESCROW_AUTHORITY_SEED, config.as_ref()], &crate::ID)
}

pub fn sol_vault(config: &Address) -> (Address, u8) {
    Address::find_program_address(&[SOL_VAULT_SEED, config.as_ref()], &crate::ID)
}
