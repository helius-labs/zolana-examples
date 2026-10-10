//! Adapter over the Kamino `kvault-interface` and `klend-interface` crates.
//!
//! Boundary rule: the Kamino crates are built on solana-pubkey 2.4 and
//! solana-instruction 2.3. Their `Pubkey` and `Instruction` are distinct types
//! from the `solana_address::Address` and solana-instruction 3.4 types the rest
//! of this workspace uses, and they never leave this module. Everything public
//! here takes and returns `Address`, `Instruction` (3.4) or plain views.
//!
//! Account readers copy the account bytes into an 8-aligned buffer before
//! casting: `kvault_interface::from_account_data` casts with
//! `bytemuck::from_bytes`, which panics on a misaligned slice.

use klend_interface::state::Reserve;
use kvault_interface::{
    helpers::{self, ReserveInfo, VaultInfo, VaultInfoError},
    state::{GlobalConfig, VaultState},
};
use solana_address::Address;
use solana_instruction::{AccountMeta, Instruction};
use solana_instruction_v2::Instruction as Instruction2;
use solana_pubkey_v2::Pubkey as Pubkey2;
use thiserror::Error;
use zolana_interface::pda;

use crate::pair::{self, Pair, ReserveSnapshot};

/// Kamino Vault program (mainnet id, also used on localnet).
pub const KVAULT_PROGRAM_ID: Address =
    Address::from_str_const("KvauGMspG5k6rtzrqqn7WNn3oZdyKqLKwK2XWQ8FLjd");
/// Capacity of a vault's allocation strategy (kvault-interface `lib.rs:118`).
pub const MAX_RESERVES: usize = kvault_interface::MAX_RESERVES;

/// Offset of the `amount` field in an SPL token account
/// (`spl_token::state::Account`: mint 32 bytes, owner 32 bytes, then the u64
/// amount, little endian). The Token-2022 base layout is the same.
pub const TOKEN_ACCOUNT_AMOUNT_OFFSET: usize = 2 * size_of::<Address>();

/// The amount of an SPL token account's data, `None` if the data is too short.
pub fn token_account_amount(data: &[u8]) -> Option<u64> {
    data.get(TOKEN_ACCOUNT_AMOUNT_OFFSET..TOKEN_ACCOUNT_AMOUNT_OFFSET + size_of::<u64>())
        .and_then(|bytes| bytes.try_into().ok())
        .map(u64::from_le_bytes)
}

/// Kamino Lending program (mainnet id, also used on localnet).
pub const KLEND_PROGRAM_ID: Address =
    Address::from_str_const("KLend2g3cP87fffoy8q1mQqGKjrxjC8boSyAYavgmjD");

/// Failures of the adapter: an account that does not decode as the expected
/// Kamino type, or a vault snapshot the instruction builders reject.
#[derive(Debug, Error)]
pub enum KvaultError {
    /// The bytes of a kVault account (`account` names the type) failed the
    /// discriminator or length check of `kvault_interface::from_account_data`.
    #[error("invalid kVault {account} account: {source}")]
    KvaultAccount {
        account: &'static str,
        source: kvault_interface::AccountDataError,
    },
    /// The bytes of a klend `Reserve` failed the discriminator or length check.
    #[error("invalid klend reserve account: {0}")]
    ReserveAccount(klend_interface::state::AccountDataError),
    /// `VaultInfo::from_vault_state` rejected the snapshot's reserves.
    #[error("kVault instruction accounts: {0}")]
    VaultInfo(VaultInfoError),
}

/// One non-empty slot of a vault's allocation strategy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Allocation {
    /// The klend reserve the slot invests in.
    pub reserve: Address,
    /// cTokens of `reserve` the vault holds, as recorded in the slot.
    pub ctoken_allocation: u64,
}

/// The `VaultState` fields the SDK reads, copied verbatim from the account
/// (amounts in token or share atoms, `_sf` fields as raw `Fraction` bits).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VaultStateView {
    pub token_mint: Address,
    pub token_vault: Address,
    pub token_program: Address,
    pub shares_mint: Address,
    pub base_vault_authority: Address,
    pub token_available: u64,
    pub shares_issued: u64,
    pub pending_fees_sf: u128,
    /// The non-empty slots of the allocation strategy, in slot order.
    pub allocations: Vec<Allocation>,
    pub min_deposit_amount: u64,
    pub min_withdraw_amount: u64,
    pub withdrawal_penalty_lamports: u64,
    pub withdrawal_penalty_bps: u64,
    /// 0 means uncapped.
    pub deposit_cap: u64,
    pub crank_fund_fee_per_reserve: u64,
    /// Slots with a reserve, a non-zero target weight and a non-zero token
    /// cap: the count the program multiplies `crank_fund_fee_per_reserve` by
    /// on deposit (kvault `state.rs:199` `get_reserves_with_allocation_count`).
    pub reserves_with_allocation: u64,
}

/// The kVault `GlobalConfig` fields the SDK reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GlobalConfigView {
    pub withdrawal_penalty_lamports: u64,
    pub withdrawal_penalty_bps: u64,
}

/// The klend `Reserve` fields the SDK reads, copied verbatim from the
/// account; the `_sf` fields are raw `Fraction` bits. They are the inputs of
/// the reserve's cToken exchange rate (see `price::collateral_to_liquidity_sf`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReserveView {
    pub lending_market: Address,
    pub liquidity_supply_vault: Address,
    pub collateral_mint: Address,
    pub total_available_amount: u64,
    pub borrowed_amount_sf: u128,
    pub accumulated_protocol_fees_sf: u128,
    pub accumulated_referrer_fees_sf: u128,
    pub pending_referrer_fees_sf: u128,
    pub collateral_mint_total_supply: u64,
}

// `Address::new_from_array(key.to_bytes())` is otherwise banned: here it is the
// only way across the two distinct key types of the 2.x and 3.x+ crates.
fn to_address(key: &Pubkey2) -> Address {
    Address::new_from_array(key.to_bytes())
}

fn to_pubkey(address: &Address) -> Pubkey2 {
    Pubkey2::new_from_array(address.to_bytes())
}

/// Copies a solana-instruction 2.3 instruction from the Kamino builders into
/// the solana-instruction 3.4 type, field by field.
fn convert_instruction(instruction: Instruction2) -> Instruction {
    Instruction {
        program_id: to_address(&instruction.program_id),
        accounts: instruction
            .accounts
            .into_iter()
            .map(|meta| AccountMeta {
                pubkey: to_address(&meta.pubkey),
                is_signer: meta.is_signer,
                is_writable: meta.is_writable,
            })
            .collect(),
        data: instruction.data,
    }
}

/// The public accounts of the party depositing into or withdrawing from a
/// vault: the signer and its token and share accounts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UserAccounts {
    pub user: Address,
    /// The signer's account of the vault's token mint.
    pub token_account: Address,
    /// The signer's account of the vault's share mint.
    pub shares_account: Address,
}

impl UserAccounts {
    /// `owner` as the signer, with its associated token accounts of `pair`.
    pub fn associated(owner: Address, pair: &Pair) -> Self {
        Self {
            user: owner,
            token_account: pda::associated_token_address(&owner, &pair.token_mint),
            shares_account: pda::associated_token_address(&owner, &pair.shares_mint),
        }
    }
}

/// The crate's `VaultInfo` for `vault`, rebuilt from the snapshot: the token
/// mint and program, and the allocated reserves with their lending markets in
/// allocation order.
///
/// `VaultInfo` only has constructors from the program's `VaultState`
/// (`from_vault_state` and `from_account_data`, kvault-interface
/// `helpers/info.rs:98`, `:135`; its reserve list is private), so the
/// snapshot's fields are written into a zeroed one. That costs one zeroed
/// 62_544-byte allocation (`size_of::<VaultState>()`, boxed to keep it off
/// the stack) per instruction build, a handful per rebalance; there is no
/// cheaper constructor. The reserves fill slots `0..n` in snapshot order,
/// which keeps the program's allocation order.
fn vault_info(vault: &Address, state: &pair::VaultState) -> Result<VaultInfo, KvaultError> {
    let mut program_state: Box<VaultState> = bytemuck::zeroed_box();
    program_state.token_mint = to_pubkey(&state.token_mint);
    program_state.token_program = to_pubkey(&state.token_program);
    program_state
        .vault_allocation_strategy
        .iter_mut()
        .zip(state.reserves.as_slice())
        .for_each(|(slot, snapshot)| slot.reserve = to_pubkey(&snapshot.reserve));
    let reserves: Vec<ReserveInfo> = state.reserves.as_slice().iter().map(reserve_info).collect();
    VaultInfo::from_vault_state(to_pubkey(vault), &program_state, &reserves)
        .map_err(KvaultError::VaultInfo)
}

fn reserve_info(snapshot: &ReserveSnapshot) -> ReserveInfo {
    ReserveInfo {
        address: to_pubkey(&snapshot.reserve),
        lending_market: to_pubkey(&snapshot.lending_market),
        liquidity_supply_vault: to_pubkey(&snapshot.liquidity_supply_vault),
        collateral_mint: to_pubkey(&snapshot.collateral_mint),
    }
}

/// The kVault `deposit` of up to `max_amount` of `user.token_account` into
/// `vault`, minting shares to `user.shares_account`.
///
/// Built by kvault-interface `helpers::deposit`: 13 fixed accounts (user
/// signer, vault, token vault, token mint, base vault authority, shares
/// mint, the user's token and share accounts, the klend program, the token
/// program from `state.token_program`, the SPL Token program for the
/// shares, the event authority and the kVault program), then the refresh
/// accounts: every reserve in `state.reserves` writable, then their lending
/// markets read-only, both in allocation order.
///
/// The reserves are appended because the program prices the deposit at the
/// current AUM: it refreshes every allocated reserve itself through a klend
/// `RefreshReservesBatch` CPI on these accounts and checks that they are the
/// vault's allocations in slot order. No oracle accounts are needed.
pub fn deposit_instruction(
    vault: &Address,
    state: &pair::VaultState,
    user: &UserAccounts,
    max_amount: u64,
) -> Result<Instruction, KvaultError> {
    Ok(convert_instruction(helpers::deposit(
        &vault_info(vault, state)?,
        to_pubkey(&user.user),
        to_pubkey(&user.token_account),
        to_pubkey(&user.shares_account),
        max_amount,
    )))
}

/// The kVault `withdraw_from_available` of `shares` from
/// `user.shares_account`, paying tokens from the vault's uninvested
/// `token_available` only (the program fails if it does not cover them).
///
/// Built by kvault-interface `helpers::withdraw_from_available`: the fixed
/// accounts (user signer, vault, global config, token vault, base vault
/// authority, the user's token account, token mint, the user's share
/// account, shares mint, token program from `state.token_program`, SPL
/// Token for the shares, the klend program, event authority and kVault
/// program), then the same refresh accounts as [`deposit_instruction`],
/// for the same reason: the program refreshes the reserves itself to price
/// the shares.
pub fn withdraw_from_available_instruction(
    vault: &Address,
    state: &pair::VaultState,
    user: &UserAccounts,
    shares: u64,
) -> Result<Instruction, KvaultError> {
    Ok(convert_instruction(helpers::withdraw_from_available(
        &vault_info(vault, state)?,
        to_pubkey(&user.user),
        to_pubkey(&user.token_account),
        to_pubkey(&user.shares_account),
        shares,
    )))
}

/// The kVault `withdraw` of `shares` from `user.shares_account`, paying
/// `token_available` first and disinvesting the rest from `reserve`.
///
/// Built by kvault-interface `helpers::withdraw`: the
/// `withdraw_from_available` accounts plus the disinvestment accounts of
/// `reserve` (the reserve, the vault's cToken vault for it, its lending
/// market and lending market authority, its liquidity supply vault and
/// collateral mint, SPL Token for the cTokens and the instructions sysvar),
/// then the refresh accounts as in [`deposit_instruction`]. Pick `reserve`
/// with [`pair::VaultState::withdraw_source`].
pub fn withdraw_instruction(
    vault: &Address,
    state: &pair::VaultState,
    user: &UserAccounts,
    reserve: &ReserveSnapshot,
    shares: u64,
) -> Result<Instruction, KvaultError> {
    Ok(convert_instruction(helpers::withdraw(
        &vault_info(vault, state)?,
        to_pubkey(&user.user),
        to_pubkey(&user.token_account),
        to_pubkey(&user.shares_account),
        &reserve_info(reserve),
        shares,
    )))
}

/// An 8-aligned copy of account bytes; the Kamino account structs have an
/// alignment of 8.
struct AlignedData {
    words: Vec<u64>,
    len: usize,
}

impl AlignedData {
    fn new(data: &[u8]) -> Self {
        let mut words = vec![0u64; data.len().div_ceil(size_of::<u64>())];
        let bytes: &mut [u8] = bytemuck::cast_slice_mut(&mut words);
        bytes
            .iter_mut()
            .zip(data)
            .for_each(|(slot, byte)| *slot = *byte);
        Self {
            words,
            len: data.len(),
        }
    }

    fn bytes(&self) -> &[u8] {
        let bytes: &[u8] = bytemuck::cast_slice(&self.words);
        bytes.get(..self.len).unwrap_or(bytes)
    }
}

/// Decodes a kVault `VaultState` account.
///
/// Errors with `KvaultError::KvaultAccount` when the discriminator or length
/// does not match.
pub fn vault_state(data: &[u8]) -> Result<VaultStateView, KvaultError> {
    let aligned = AlignedData::new(data);
    let state =
        kvault_interface::from_account_data::<VaultState>(aligned.bytes()).map_err(|source| {
            KvaultError::KvaultAccount {
                account: "VaultState",
                source,
            }
        })?;
    let empty = Pubkey2::default();
    let reserves_with_allocation = state
        .vault_allocation_strategy
        .iter()
        .filter(|slot| {
            slot.reserve != empty
                && slot.target_allocation_weight > 0
                && slot.token_allocation_cap > 0
        })
        .count();
    let allocations = state
        .vault_allocation_strategy
        .iter()
        .filter(|slot| slot.reserve != empty)
        .map(|slot| Allocation {
            reserve: to_address(&slot.reserve),
            ctoken_allocation: slot.ctoken_allocation,
        })
        .collect();
    Ok(VaultStateView {
        token_mint: to_address(&state.token_mint),
        token_vault: to_address(&state.token_vault),
        token_program: to_address(&state.token_program),
        shares_mint: to_address(&state.shares_mint),
        base_vault_authority: to_address(&state.base_vault_authority),
        token_available: state.token_available,
        shares_issued: state.shares_issued,
        pending_fees_sf: state.pending_fees_sf.into(),
        allocations,
        min_deposit_amount: state.min_deposit_amount,
        min_withdraw_amount: state.min_withdraw_amount,
        withdrawal_penalty_lamports: state.withdrawal_penalty_lamports,
        withdrawal_penalty_bps: state.withdrawal_penalty_bps,
        deposit_cap: state.deposit_cap,
        crank_fund_fee_per_reserve: state.crank_fund_fee_per_reserve,
        // At most `MAX_RESERVES` (25) slots, so the count fits a `u64`.
        reserves_with_allocation: u64::try_from(reserves_with_allocation).unwrap_or(u64::MAX),
    })
}

/// Decodes the kVault `GlobalConfig` account.
///
/// Errors with `KvaultError::KvaultAccount` when the discriminator or length
/// does not match.
pub fn global_config(data: &[u8]) -> Result<GlobalConfigView, KvaultError> {
    let aligned = AlignedData::new(data);
    let config =
        kvault_interface::from_account_data::<GlobalConfig>(aligned.bytes()).map_err(|source| {
            KvaultError::KvaultAccount {
                account: "GlobalConfig",
                source,
            }
        })?;
    Ok(GlobalConfigView {
        withdrawal_penalty_lamports: config.withdrawal_penalty_lamports,
        withdrawal_penalty_bps: config.withdrawal_penalty_bps,
    })
}

/// Decodes a klend `Reserve` account.
///
/// Errors with `KvaultError::ReserveAccount` when the discriminator or length
/// does not match.
pub fn reserve(data: &[u8]) -> Result<ReserveView, KvaultError> {
    let aligned = AlignedData::new(data);
    let reserve = klend_interface::state::from_account_data::<Reserve>(aligned.bytes())
        .map_err(KvaultError::ReserveAccount)?;
    Ok(ReserveView {
        lending_market: to_address(&reserve.lending_market),
        liquidity_supply_vault: to_address(&reserve.liquidity.supply_vault),
        collateral_mint: to_address(&reserve.collateral.mint_pubkey),
        total_available_amount: reserve.liquidity.total_available_amount,
        borrowed_amount_sf: reserve.liquidity.borrowed_amount_sf.into(),
        accumulated_protocol_fees_sf: reserve.liquidity.accumulated_protocol_fees_sf.into(),
        accumulated_referrer_fees_sf: reserve.liquidity.accumulated_referrer_fees_sf.into(),
        pending_referrer_fees_sf: reserve.liquidity.pending_referrer_fees_sf.into(),
        collateral_mint_total_supply: reserve.collateral.mint_total_supply,
    })
}

/// The vault's base authority PDA, seeds `[b"authority", vault]`.
pub fn base_vault_authority(vault: &Address) -> Address {
    to_address(
        &kvault_interface::pda::base_vault_authority(
            &kvault_interface::KVAULT_PROGRAM_ID,
            &to_pubkey(vault),
        )
        .0,
    )
}

/// The vault's token account PDA, seeds `[b"token_vault", vault]`.
pub fn token_vault(vault: &Address) -> Address {
    to_address(
        &kvault_interface::pda::token_vault(
            &kvault_interface::KVAULT_PROGRAM_ID,
            &to_pubkey(vault),
        )
        .0,
    )
}

/// The vault's share mint PDA, seeds `[b"shares", vault]`.
pub fn shares_mint(vault: &Address) -> Address {
    to_address(
        &kvault_interface::pda::shares_mint(
            &kvault_interface::KVAULT_PROGRAM_ID,
            &to_pubkey(vault),
        )
        .0,
    )
}

/// The vault's cToken account PDA for `reserve`, seeds
/// `[b"ctoken_vault", vault, reserve]`.
pub fn ctoken_vault(vault: &Address, reserve: &Address) -> Address {
    to_address(
        &kvault_interface::pda::ctoken_vault(
            &kvault_interface::KVAULT_PROGRAM_ID,
            &to_pubkey(vault),
            &to_pubkey(reserve),
        )
        .0,
    )
}

/// The kVault global config PDA, seeds `[b"global_config"]`.
pub fn global_config_address() -> Address {
    to_address(&kvault_interface::pda::global_config(&kvault_interface::KVAULT_PROGRAM_ID).0)
}

/// The klend lending market authority PDA, seeds `[b"lma", market]`.
pub fn lending_market_authority(market: &Address) -> Address {
    to_address(
        &klend_interface::pda::lending_market_authority(
            &klend_interface::KLEND_PROGRAM_ID,
            &to_pubkey(market),
        )
        .0,
    )
}

#[cfg(test)]
mod tests {
    use kvault_interface::state::SplDiscriminate;
    use solana_instruction_v2::AccountMeta as AccountMeta2;

    use super::*;
    use crate::pair::Reserves;

    /// The program ids hard-coded here are the ids the interface crates
    /// export.
    #[test]
    fn program_ids_match_the_crates() {
        assert_eq!(
            KVAULT_PROGRAM_ID,
            to_address(&kvault_interface::KVAULT_PROGRAM_ID)
        );
        assert_eq!(
            KLEND_PROGRAM_ID,
            to_address(&klend_interface::KLEND_PROGRAM_ID)
        );
    }

    /// A zeroed `VaultState` parses to a view with no allocations.
    #[test]
    fn zeroed_vault_state_has_no_allocations() {
        assert_eq!(
            vault_state(&zeroed_vault_account()).ok(),
            Some(zeroed_view())
        );
    }

    /// `vault_state` parses account bytes that start at an odd address.
    #[test]
    fn misaligned_vault_state_parses() {
        let mut buffer = vec![0u8];
        buffer.extend(zeroed_vault_account());
        let misaligned = buffer.get(1..).unwrap_or_default();
        assert_eq!(vault_state(misaligned).ok(), Some(zeroed_view()));
    }

    /// `base_vault_authority` is the kVault PDA of `[b"authority", vault]`.
    #[test]
    fn base_vault_authority_matches_seeds() {
        let vault = Address::new_unique();
        assert_eq!(
            base_vault_authority(&vault),
            Address::find_program_address(&[b"authority", vault.as_ref()], &KVAULT_PROGRAM_ID).0
        );
    }

    /// A deposit carries its 13 fixed accounts, then the reserves writable
    /// and their lending markets read-only, in allocation order.
    #[test]
    fn deposit_appends_reserves_then_lending_markets() {
        let (a, b) = (snapshot(), snapshot());
        let state = two_reserve_state(a, b);
        let vault = Address::new_unique();
        let user = user_accounts();
        let instruction = deposit_instruction(&vault, &state, &user, 500);
        let accounts = instruction.map(|instruction| instruction.accounts);
        assert_eq!(accounts.as_ref().map(Vec::len).ok(), Some(17));
        assert_eq!(
            accounts
                .as_ref()
                .ok()
                .and_then(|accounts| accounts.get(..3))
                .map(<[AccountMeta]>::to_vec),
            Some(vec![
                AccountMeta::new(user.user, true),
                AccountMeta::new(vault, false),
                AccountMeta::new(token_vault(&vault), false),
            ])
        );
        assert_eq!(
            accounts
                .as_ref()
                .ok()
                .and_then(|accounts| accounts.get(13..))
                .map(<[AccountMeta]>::to_vec),
            Some(vec![
                AccountMeta::new(a.reserve, false),
                AccountMeta::new(b.reserve, false),
                AccountMeta::new_readonly(a.lending_market, false),
                AccountMeta::new_readonly(b.lending_market, false),
            ])
        );
        assert!(accounts.is_ok_and(|accounts| accounts
            .iter()
            .any(|meta| meta.pubkey == state.token_program && !meta.is_writable)));
    }

    /// A withdraw against reserve `b` names `b`'s cToken vault and lending
    /// market authority, and ends with the same refresh accounts.
    #[test]
    fn withdraw_against_a_reserve_names_its_accounts() {
        let (a, b) = (snapshot(), snapshot());
        let state = two_reserve_state(a, b);
        let vault = Address::new_unique();
        let user = user_accounts();
        let accounts = withdraw_instruction(&vault, &state, &user, &b, 500)
            .map(|instruction| instruction.accounts)
            .unwrap_or_default();
        let contains = |pubkey: Address| accounts.iter().any(|meta| meta.pubkey == pubkey);
        assert!(contains(ctoken_vault(&vault, &b.reserve)));
        assert!(contains(lending_market_authority(&b.lending_market)));
        assert!(contains(b.liquidity_supply_vault));
        assert!(contains(b.collateral_mint));
        assert!(!contains(ctoken_vault(&vault, &a.reserve)));
        assert_eq!(
            accounts
                .len()
                .checked_sub(4)
                .and_then(|tail| accounts.get(tail..)),
            Some(
                [
                    AccountMeta::new(a.reserve, false),
                    AccountMeta::new(b.reserve, false),
                    AccountMeta::new_readonly(a.lending_market, false),
                    AccountMeta::new_readonly(b.lending_market, false),
                ]
                .as_slice()
            )
        );
        let from_available = withdraw_from_available_instruction(&vault, &state, &user, 500)
            .map(|instruction| instruction.accounts.len());
        assert_eq!(from_available.ok(), Some(18));
    }

    /// `convert_instruction` keeps the program id, every account meta and
    /// the data of a v2 instruction.
    #[test]
    fn convert_instruction_preserves_fields() {
        let program_id = Address::new_unique();
        let writable_signer = Address::new_unique();
        let readonly = Address::new_unique();
        let converted = convert_instruction(Instruction2 {
            program_id: to_pubkey(&program_id),
            accounts: vec![
                AccountMeta2::new(to_pubkey(&writable_signer), true),
                AccountMeta2::new_readonly(to_pubkey(&readonly), false),
            ],
            data: vec![1, 2, 3],
        });
        assert_eq!(
            converted,
            Instruction {
                program_id,
                accounts: vec![
                    AccountMeta::new(writable_signer, true),
                    AccountMeta::new_readonly(readonly, false),
                ],
                data: vec![1, 2, 3],
            }
        );
    }

    // Test fixtures and shared helpers.

    /// A `VaultState` account of zeroes behind the real discriminator.
    fn zeroed_vault_account() -> Vec<u8> {
        let mut data = vec![0u8; 8 + core::mem::size_of::<VaultState>()];
        if let Some(discriminator) = data.get_mut(..8) {
            discriminator.copy_from_slice(VaultState::SPL_DISCRIMINATOR_SLICE);
        }
        data
    }

    /// The view `vault_state` parses from `zeroed_vault_account`.
    fn zeroed_view() -> VaultStateView {
        VaultStateView {
            token_mint: Address::default(),
            token_vault: Address::default(),
            token_program: Address::default(),
            shares_mint: Address::default(),
            base_vault_authority: Address::default(),
            token_available: 0,
            shares_issued: 0,
            pending_fees_sf: 0,
            allocations: Vec::new(),
            min_deposit_amount: 0,
            min_withdraw_amount: 0,
            withdrawal_penalty_lamports: 0,
            withdrawal_penalty_bps: 0,
            deposit_cap: 0,
            crank_fund_fee_per_reserve: 0,
            reserves_with_allocation: 0,
        }
    }

    /// A reserve allocation with fresh addresses and 1_000 cTokens.
    fn snapshot() -> ReserveSnapshot {
        ReserveSnapshot {
            reserve: Address::new_unique(),
            lending_market: Address::new_unique(),
            liquidity_supply_vault: Address::new_unique(),
            collateral_mint: Address::new_unique(),
            ctoken_allocation: 1_000,
            invested_liquidity_sf: 0,
        }
    }

    /// A vault snapshot allocating to reserves `a` and `b`, in that order.
    fn two_reserve_state(a: ReserveSnapshot, b: ReserveSnapshot) -> pair::VaultState {
        let mut reserves = Reserves::EMPTY;
        assert_eq!(reserves.push(a), Ok(()));
        assert_eq!(reserves.push(b), Ok(()));
        pair::VaultState {
            token_mint: Address::new_unique(),
            token_program: Address::new_unique(),
            token_available: 0,
            shares_issued: 0,
            pending_fees_sf: 0,
            reserves,
            min_deposit_amount: 0,
            min_withdraw_amount: 0,
            deposit_cap: 0,
            crank_funds: 0,
            withdrawal_penalty_lamports: 0,
            withdrawal_penalty_bps: 0,
            global_withdrawal_penalty_lamports: 0,
            global_withdrawal_penalty_bps: 0,
        }
    }

    /// A user and its token and share accounts, all fresh addresses.
    fn user_accounts() -> UserAccounts {
        UserAccounts {
            user: Address::new_unique(),
            token_account: Address::new_unique(),
            shares_account: Address::new_unique(),
        }
    }
}
