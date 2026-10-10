use anyhow::{anyhow, Result};
use solana_account::Account;
use solana_address::Address;
use solana_instruction::{AccountMeta, Instruction};
use zolana_client::{Rpc, SolanaRpc};
use zolana_interface::pda::spl_token_program_id;

use k_lend_rfq_sdk::{
    kvault::{token_account_amount, KVAULT_PROGRAM_ID},
    pair::Pair,
};

/// Byte length of a kVault `VaultState` account: the 8-byte Anchor account
/// discriminator plus `size_of::<VaultState>() == 62544`, asserted in
/// kvault-interface `src/state/vault_state.rs:124`.
pub const VAULT_STATE_SIZE: usize = 8 + 62544;
const SYSVAR_RENT_ID: Address =
    Address::from_str_const("SysvarRent111111111111111111111111111111111");
/// Byte length of the kVault `GlobalConfig` account: the 8-byte Anchor
/// account discriminator plus `size_of::<GlobalConfig>() == 1024`, asserted in
/// kvault-interface `src/state/global_config.rs:24`.
const GLOBAL_CONFIG_SIZE: usize = 8 + 1024;
/// `GlobalConfig::global_admin` (`Pubkey`, kvault-interface
/// `src/state/global_config.rs:11`), the first field after the discriminator.
const GLOBAL_ADMIN_OFFSET: usize = 8;
/// `GlobalConfig::pending_admin` (`Pubkey`, kvault-interface
/// `src/state/global_config.rs:13`), right after `global_admin`.
const PENDING_ADMIN_OFFSET: usize = GLOBAL_ADMIN_OFFSET + 32;
/// Lamports written to the `GlobalConfig` account: 1 SOL, above its
/// rent-exempt minimum, so the runtime never treats it as rent-paying.
const GLOBAL_CONFIG_LAMPORTS: u64 = 1_000_000_000;
/// First 8 bytes of `sha256("account:GlobalConfig")`, the Anchor account
/// discriminator of `GlobalConfig` (kvault-interface
/// `src/state/global_config.rs:7`).
const GLOBAL_CONFIG_DISCRIMINATOR: [u8; 8] = [0x95, 0x08, 0x9c, 0xca, 0xa0, 0xfc, 0xb0, 0xd9];
/// First 8 bytes of `sha256("global:init_vault")`, the Anchor instruction
/// discriminator of kVault `init_vault`. kvault-interface does not define it;
/// `discriminators::compute_discriminator("init_vault")` produces it.
const INIT_VAULT_DISCRIMINATOR: [u8; 8] = [0x4d, 0x4f, 0x55, 0x96, 0x21, 0xd9, 0x34, 0x6a];

/// A `GlobalConfig` account with `admin` as global and pending admin.
/// Errors if a field range lies outside `GLOBAL_CONFIG_SIZE` bytes.
pub fn global_config_account(admin: &Address) -> Result<Account> {
    let mut account_data = vec![0u8; GLOBAL_CONFIG_SIZE];
    for (range, bytes) in [
        (0..8, GLOBAL_CONFIG_DISCRIMINATOR.as_slice()),
        (
            GLOBAL_ADMIN_OFFSET..GLOBAL_ADMIN_OFFSET + 32,
            admin.as_ref(),
        ),
        (
            PENDING_ADMIN_OFFSET..PENDING_ADMIN_OFFSET + 32,
            admin.as_ref(),
        ),
    ] {
        account_data
            .get_mut(range.clone())
            .ok_or_else(|| {
                anyhow!("GlobalConfig field {range:?} outside {GLOBAL_CONFIG_SIZE} bytes")
            })?
            .copy_from_slice(bytes);
    }
    Ok(Account {
        lamports: GLOBAL_CONFIG_LAMPORTS,
        data: account_data,
        owner: KVAULT_PROGRAM_ID,
        executable: false,
        rent_epoch: 0,
    })
}

pub struct InitVault {
    pub admin: Address,
    pub admin_token_account: Address,
    pub pair: Pair,
}

impl InitVault {
    pub fn instruction(&self) -> Instruction {
        let token_program = spl_token_program_id();
        Instruction {
            program_id: KVAULT_PROGRAM_ID,
            accounts: vec![
                AccountMeta::new(self.admin, true),
                AccountMeta::new(self.pair.vault, true),
                AccountMeta::new_readonly(self.pair.authority, false),
                AccountMeta::new(self.pair.token_vault, false),
                AccountMeta::new_readonly(self.pair.token_mint, false),
                AccountMeta::new(self.pair.shares_mint, false),
                AccountMeta::new(self.admin_token_account, false),
                // The system program.
                AccountMeta::new_readonly(Address::default(), false),
                AccountMeta::new_readonly(SYSVAR_RENT_ID, false),
                AccountMeta::new_readonly(token_program, false),
                AccountMeta::new_readonly(token_program, false),
            ],
            data: INIT_VAULT_DISCRIMINATOR.to_vec(),
        }
    }
}

pub fn token_balance(rpc: &SolanaRpc, account: &Address) -> Result<u64> {
    let account_data = rpc
        .get_account(*account)?
        .ok_or_else(|| anyhow!("account {account} missing"))?
        .data;
    token_account_amount(&account_data).ok_or_else(|| anyhow!("token account {account} too short"))
}

#[cfg(test)]
mod tests {
    //! Tested invariants:
    //! 1. The hard-coded Anchor discriminators are the first 8 bytes of the
    //!    sha256 of their Anchor preimage.
    //! 2. `VAULT_STATE_SIZE` is the exact length the SDK parser accepts for a
    //!    `VaultState` account: it parses at that length and fails with
    //!    `DataTooShort` one byte below it.
    //! 3. `global_config_account` builds a `GlobalConfig` account of exactly
    //!    `GLOBAL_CONFIG_SIZE` bytes, the length the SDK parser accepts.

    use k_lend_rfq_sdk::kvault::{self, KvaultError};
    use kvault_interface::AccountDataError;
    use sha2::{Digest, Sha256};

    use super::*;

    /// Invariant 1: each discriminator equals the sha256 prefix of its
    /// preimage.
    #[test]
    fn discriminators_match_sha256() {
        for (label, discriminator, preimage) in [
            ("init_vault", INIT_VAULT_DISCRIMINATOR, "global:init_vault"),
            (
                "GlobalConfig",
                GLOBAL_CONFIG_DISCRIMINATOR,
                "account:GlobalConfig",
            ),
        ] {
            let want = sha256_prefix(preimage);
            assert_eq!(
                discriminator, want,
                "{label}: got {discriminator:?}, want {want:?}"
            );
        }
    }

    /// Invariant 2: `from_account_data` rejects data shorter than
    /// discriminator plus struct, so parsing at `len` but not at `len - 1`
    /// proves `len` exact.
    #[test]
    fn vault_state_size_matches_kvault() {
        let mut account_data = vec![0u8; VAULT_STATE_SIZE];
        account_data
            .get_mut(..8)
            .expect("discriminator slot")
            .copy_from_slice(&sha256_prefix("account:VaultState"));
        kvault::vault_state(&account_data).expect("a VaultState of VAULT_STATE_SIZE bytes parses");
        let shorter = account_data.get(..VAULT_STATE_SIZE - 1).expect("shorter");
        assert_too_short(kvault::vault_state(shorter), "VaultState", VAULT_STATE_SIZE);
    }

    /// Invariant 3: the built `GlobalConfig` is `GLOBAL_CONFIG_SIZE` bytes,
    /// parses, and fails with `DataTooShort` one byte shorter.
    #[test]
    fn global_config_size_matches_kvault() {
        let account = global_config_account(&Address::new_from_array([7; 32]))
            .expect("the GlobalConfig fields fit");
        assert_eq!(
            account.data.len(),
            GLOBAL_CONFIG_SIZE,
            "global config length: got {}, want {GLOBAL_CONFIG_SIZE}",
            account.data.len()
        );
        kvault::global_config(&account.data)
            .expect("a GlobalConfig of GLOBAL_CONFIG_SIZE bytes parses");
        let shorter = account.data.get(..GLOBAL_CONFIG_SIZE - 1).expect("shorter");
        assert_too_short(
            kvault::global_config(shorter),
            "GlobalConfig",
            GLOBAL_CONFIG_SIZE,
        );
    }

    // Test fixtures and shared helpers.

    /// The first 8 bytes of `sha256(preimage)`.
    fn sha256_prefix(preimage: &str) -> [u8; 8] {
        let hash = Sha256::digest(preimage.as_bytes());
        hash.get(..8)
            .expect("sha256 is 32 bytes")
            .try_into()
            .expect("8-byte prefix")
    }

    /// Asserts that parsing `account` from `size - 1` bytes failed with
    /// `DataTooShort { expected: size, actual: size - 1 }`.
    #[track_caller]
    fn assert_too_short<T: std::fmt::Debug>(
        parsed: Result<T, KvaultError>,
        account: &str,
        size: usize,
    ) {
        assert!(
            matches!(
                &parsed,
                Err(KvaultError::KvaultAccount {
                    account: parsed_account,
                    source: AccountDataError::DataTooShort { expected, actual },
                }) if *parsed_account == account && *expected == size && *actual == size - 1
            ),
            "got {parsed:?}, want KvaultAccount {{ {account}, DataTooShort {{ {size}, {} }} }}",
            size - 1
        );
    }
}
