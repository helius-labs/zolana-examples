//! Shared Mollusk harness and account fixtures for the staking program tests.
//!
//! `Harness::new` loads the staking program, the shielded pool program (SPP),
//! and the mainnet SPL stake pool, SPL token, and stake programs dumped by
//! `scripts/dump-stake-pool.sh`. Every account an instruction references is
//! resolved by `Harness::accounts_for`: program ids become executable program
//! accounts, sysvar ids come from Mollusk's sysvar cache, fixtures come from
//! `Harness::accounts`, and any other key is an empty system wallet.
#![allow(dead_code)]

use std::{collections::BTreeMap, path::Path, sync::Once};

use mollusk_svm::{result::InstructionResult, Mollusk};
use num_bigint::BigUint;
use private_staking_program::{
    error::StakingError,
    instructions::{
        execute::stake_program,
        management::{CreateBatchIxData, CreateConfigIxData},
    },
    math::PoolSnapshot,
    pda, pool,
    state::{self, Config, EpochHeader},
    ID,
};
use private_staking_sdk::instructions;
use solana_account::Account;
use solana_address::Address;
use solana_instruction::{error::InstructionError, Instruction};
use solana_program_option::COption;
use solana_program_pack::Pack;
use solana_stake_interface::state::{Authorized, Lockup, Meta, StakeStateV2};
use spl_stake_pool::state::{AccountType, Fee, StakePool};
use spl_token_2022_interface::state::{Account as TokenAccount, AccountState, Mint as TokenMint};
use zolana_client::{
    MerkleContext, MerkleProof, NonInclusionProof, ProverLaunch, SpendProof, NULLIFIER_TREE_HEIGHT,
    STATE_TREE_HEIGHT,
};
use zolana_gnark_ffi_prover::Circuit;
use zolana_hasher::Poseidon;
use zolana_interface::{
    state::{
        default_tree_fees, discriminator::TREE_ACCOUNT_DISCRIMINATOR, nullifier_tree_params,
        tree_account_size, NullifierPda, NULLIFIER_PDA_SIZE, STATE_HEIGHT,
    },
    SHIELDED_POOL_CPI_AUTHORITY, SHIELDED_POOL_PROGRAM_ID, SOL_INTERFACE,
};
use zolana_merkle_tree::{indexed::IndexedMerkleTree, MerkleTree};
use zolana_transaction::instructions::transact::BN254_MODULUS_DEC;
use zolana_tree::TreeAccount;

/// Epoch the harness clock starts in; settlement happens in `EPOCH + 1`.
pub const EPOCH: u64 = 42;
pub const SPP: Address = Address::new_from_array(SHIELDED_POOL_PROGRAM_ID);
pub const TOKEN_PROGRAM: Address =
    Address::from_str_const("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA");
pub const SYSTEM_PROGRAM: Address = Address::new_from_array([0; 32]);
pub const SOL_INTERFACE_KEY: Address = Address::new_from_array(SOL_INTERFACE);
pub const SPP_CPI_AUTHORITY: Address = Address::new_from_array(SHIELDED_POOL_CPI_AUTHORITY);
/// Lamports of accounts written by `put_config` and `put_epoch`.
pub const STATE_ACCOUNT_LAMPORTS: u64 = 1_000_000_000;
/// Lamports of an escrow tree account; the SPP charges tree fees from it.
pub const TREE_LAMPORTS: u64 = 1_000_000_000_000;
/// Decimals of the fabricated pool mint.
pub const POOL_MINT_DECIMALS: u8 = 9;

const STAKE_POOL_KEY: Address = Address::new_from_array([0x51; 32]);
const RESERVE_KEY: Address = Address::new_from_array([0x52; 32]);
const POOL_MINT_KEY: Address = Address::new_from_array([0x53; 32]);
const MANAGER_FEE_KEY: Address = Address::new_from_array([0x54; 32]);
const POOL_MANAGER: Address = Address::new_from_array([0x55; 32]);
const VALIDATOR_LIST_KEY: Address = Address::new_from_array([0x56; 32]);

fn root() -> &'static Path {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/.."))
}

fn binary(relative: &str) -> String {
    root().join(relative).to_string_lossy().into_owned()
}

/// An empty system-owned account holding `lamports`.
pub fn wallet(lamports: u64) -> Account {
    Account {
        lamports,
        owner: SYSTEM_PROGRAM,
        ..Account::default()
    }
}

/// Generate any missing staking circuit keys and spawn the SPP prover server
/// once per test process.
pub fn start_prover() {
    static START: Once = Once::new();
    START.call_once(|| {
        for circuit in <private_staking_prover::CircuitId as Circuit>::ALL {
            let dir = private_staking_prover::PROVER.keys_dir(*circuit);
            if !dir.join("pk.bin").exists() {
                private_staking_prover::PROVER
                    .setup_insecure_test_keys(*circuit, &dir)
                    .unwrap();
            }
        }
        std::env::set_var("ZOLANA_PROVER_URL", "http://127.0.0.1:9873");
        std::env::set_var(
            "ZOLANA_PROVER_BIN",
            root().join(".deps/zolana/target/debug/zolana-prover"),
        );
        ProverLaunch::new_with_cli(root().join(".deps/zolana/target/debug/zolana"))
            .unwrap()
            .with_keys_dir(root().join("build/spp-keys"))
            .unwrap()
            .spawn()
            .unwrap();
    });
}

/// An escrow tree account with `leaves` appended in one batch at slot 1, and
/// the reference state tree with the same leaves.
pub struct TreeFixture {
    pub account: Account,
    pub state: MerkleTree<Poseidon>,
    /// Root history index of the current UTXO root (0 for an empty tree).
    pub root_index: u16,
}

pub fn tree_fixture(tree: Address, leaves: &[[u8; 32]]) -> TreeFixture {
    let mut bytes = vec![0; tree_account_size()];
    let (utxo_root, root_index, nullifier_root) = {
        let mut account = TreeAccount::init(
            &mut bytes,
            TREE_ACCOUNT_DISCRIMINATOR,
            STATE_HEIGHT as u8,
            tree.to_bytes(),
            0,
            nullifier_tree_params(),
            default_tree_fees(nullifier_tree_params().input_queue_zkp_batch_size).unwrap(),
        )
        .unwrap();
        if !leaves.is_empty() {
            account.utxo_tree().append_batch(leaves, 1).unwrap();
        }
        let root_index = account.utxo_tree().current_root_index();
        (
            account.get_utxo_tree_root(root_index).unwrap(),
            root_index,
            account.get_nullifier_tree_root(0).unwrap(),
        )
    };
    let mut state = MerkleTree::<Poseidon>::new(STATE_TREE_HEIGHT, 0);
    for leaf in leaves {
        state.append(leaf).unwrap();
    }
    assert_eq!(state.root(), utxo_root);
    assert_eq!(fresh_nullifier_tree().root(), nullifier_root);
    TreeFixture {
        account: Account {
            lamports: TREE_LAMPORTS,
            data: bytes,
            owner: SPP,
            ..Account::default()
        },
        state,
        root_index,
    }
}

fn fresh_nullifier_tree() -> IndexedMerkleTree<Poseidon, usize> {
    let modulus = BigUint::parse_bytes(BN254_MODULUS_DEC.as_bytes(), 10).unwrap() - 1u32;
    IndexedMerkleTree::<Poseidon, usize>::new_with_next_value(NULLIFIER_TREE_HEIGHT, 0, modulus)
        .unwrap()
}

/// Proof that `nullifier` is absent from the tree's empty nullifier tree
/// (root index 0).
pub fn non_inclusion_proof(tree: Address, nullifier: [u8; 32]) -> NonInclusionProof {
    let nullifiers = fresh_nullifier_tree();
    let absence = nullifiers
        .get_non_inclusion_proof(&BigUint::from_bytes_be(&nullifier))
        .unwrap();
    NonInclusionProof {
        leaf: nullifier,
        merkle_context: MerkleContext { tree_type: 0, tree },
        path: absence.merkle_proof.to_vec(),
        low_element: absence.leaf_lower_range_value,
        low_element_index: absence.leaf_index as u64,
        high_element: absence.leaf_higher_range_value,
        high_element_index: 0,
        root: nullifiers.root(),
        root_seq: 0,
        root_index: 0,
    }
}

/// Inclusion proof of leaf `leaf_index` of `state` at `root_index`, plus the
/// non-inclusion proof of its nullifier.
pub fn spend_proof(
    tree: Address,
    state: &MerkleTree<Poseidon>,
    root_index: u16,
    leaf_index: usize,
    nullifier: [u8; 32],
) -> SpendProof {
    SpendProof {
        state: MerkleProof {
            leaf: state.get_leaf(leaf_index).unwrap(),
            merkle_context: MerkleContext { tree_type: 0, tree },
            path: state.get_proof_of_leaf(leaf_index, true).unwrap().to_vec(),
            leaf_index: leaf_index as u64,
            root: state.root(),
            root_seq: 0,
            root_index,
        },
        nullifier: non_inclusion_proof(tree, nullifier),
    }
}

impl TreeFixture {
    pub fn spend_proof(&self, tree: Address, leaf_index: usize, nullifier: [u8; 32]) -> SpendProof {
        spend_proof(tree, &self.state, self.root_index, leaf_index, nullifier)
    }
}

/// Arguments of `create_config`.
pub struct ConfigParams {
    pub authority: Address,
    pub payer: Address,
    pub stake_pool: Address,
    pub escrow_tree: Address,
    pub data: CreateConfigIxData,
}

/// Parameters of a fabricated SPL stake pool. `reserve_extra_lamports` is the
/// reserve balance above its rent-exempt minimum, the most `WithdrawSol` pays.
#[derive(Clone, Copy, Debug)]
pub struct StakePoolParams {
    pub total_lamports: u64,
    pub pool_token_supply: u64,
    pub deposit_fee: Fee,
    pub withdrawal_fee: Fee,
    pub reserve_extra_lamports: u64,
    pub last_update_epoch: u64,
}

/// Keys of a fabricated stake pool, all present in `Harness::accounts`.
#[derive(Clone, Copy, Debug)]
pub struct StakePoolFixture {
    pub pool: Address,
    pub withdraw_authority: Address,
    pub reserve: Address,
    pub mint: Address,
    pub manager_fee: Address,
    pub token_program: Address,
    pub snapshot: PoolSnapshot,
}

#[derive(Clone, Copy, Debug)]
pub struct SppInterfaces {
    pub sol_interface: Address,
    pub spl_interface: Address,
}

pub struct Harness {
    pub svm: Mollusk,
    pub accounts: BTreeMap<Address, Account>,
    /// Programs loaded into `svm`; resolved as executable accounts.
    pub programs: Vec<Address>,
    /// Config set by `register` or the caller.
    pub config: Address,
    /// Epoch page set by `create_epoch` or the caller.
    pub epoch: Address,
}

impl Default for Harness {
    fn default() -> Self {
        Self::new()
    }
}

impl Harness {
    /// Load all programs and set the clock to the first slot of `EPOCH` at
    /// unix time 500.
    pub fn new() -> Self {
        let mut svm = Mollusk::new(&ID, &binary("target/deploy/private_staking_program"));
        let programs = vec![
            ID,
            SPP,
            spl_stake_pool::id(),
            TOKEN_PROGRAM,
            stake_program::ID,
        ];
        for (program, path) in [
            (SPP, ".deps/zolana/target/deploy/shielded_pool_program"),
            (spl_stake_pool::id(), "target/deploy/spl_stake_pool"),
            (TOKEN_PROGRAM, "target/deploy/spl_token"),
            (stake_program::ID, "target/deploy/solana_stake_program"),
        ] {
            svm.add_program(&program, &binary(path));
        }
        svm.sysvars.clock.epoch = EPOCH;
        svm.sysvars.clock.slot = svm.sysvars.epoch_schedule.get_first_slot_in_epoch(EPOCH);
        svm.sysvars.clock.unix_timestamp = 500;
        Self {
            svm,
            accounts: BTreeMap::new(),
            programs,
            config: Address::default(),
            epoch: Address::default(),
        }
    }

    pub fn rent(&self, data_len: usize) -> u64 {
        self.svm.sysvars.rent.minimum_balance(data_len)
    }

    /// Resolve every distinct account of `ix`.
    pub fn accounts_for(&self, ix: &Instruction) -> Vec<(Address, Account)> {
        let keys: std::collections::BTreeSet<_> =
            ix.accounts.iter().map(|meta| meta.pubkey).collect();
        let sysvars = self.svm.sysvars.get_all_keyed_sysvar_accounts();
        keys.into_iter()
            .map(|key| {
                if key == SYSTEM_PROGRAM {
                    mollusk_svm::program::keyed_account_for_system_program()
                } else if self.programs.contains(&key) {
                    (
                        key,
                        mollusk_svm::program::create_program_account_loader_v3(&key),
                    )
                } else if let Some(sysvar) = sysvars.iter().find(|(id, _)| *id == key) {
                    sysvar.clone()
                } else {
                    (
                        key,
                        self.accounts
                            .get(&key)
                            .cloned()
                            .unwrap_or_else(|| wallet(0)),
                    )
                }
            })
            .collect()
    }

    fn is_fixture_key(&self, key: &Address) -> bool {
        *key != SYSTEM_PROGRAM
            && !self.programs.contains(key)
            && !self
                .svm
                .sysvars
                .get_all_keyed_sysvar_accounts()
                .iter()
                .any(|(id, _)| id == key)
    }

    /// Process `ix`, assert its result, and keep the resulting accounts on
    /// success.
    pub fn run(
        &mut self,
        ix: Instruction,
        expected: Result<(), InstructionError>,
    ) -> InstructionResult {
        let accounts = self.accounts_for(&ix);
        let result = self.svm.process_instruction(&ix, &accounts);
        assert_eq!(result.raw_result, expected);
        if result.raw_result.is_ok() {
            let updates: Vec<_> = result
                .resulting_accounts
                .iter()
                .filter(|(key, _)| self.is_fixture_key(key))
                .cloned()
                .collect();
            self.accounts.extend(updates);
        }
        result
    }

    pub fn error(&mut self, ix: Instruction, error: StakingError) -> InstructionResult {
        self.run(ix, Err(InstructionError::Custom(error as u32)))
    }

    pub fn account(&self, key: &Address) -> &Account {
        self.accounts
            .get(key)
            .unwrap_or_else(|| panic!("missing account {key}"))
    }

    pub fn lamports(&self, key: &Address) -> u64 {
        self.accounts.get(key).map_or(0, |account| account.lamports)
    }

    /// Move the clock to the first slot of `EPOCH + 1`, which started at unix
    /// time 1000, and set the time to `timestamp`.
    pub fn settlement_time(&mut self, timestamp: i64) {
        self.svm.sysvars.clock.epoch = EPOCH + 1;
        self.svm.sysvars.clock.slot = self
            .svm
            .sysvars
            .epoch_schedule
            .get_first_slot_in_epoch(EPOCH + 1);
        self.svm.sysvars.clock.epoch_start_timestamp = 1_000;
        self.svm.sysvars.clock.unix_timestamp = timestamp;
    }

    /// Run `create_config`, funding the config's rent payer PDA with 1 SOL if
    /// it does not exist yet, and remember the config key.
    pub fn register(&mut self, params: ConfigParams) -> Address {
        let config = pda::config(&params.stake_pool).0;
        self.accounts
            .entry(pda::rent_payer(&config).0)
            .or_insert_with(|| wallet(1_000_000_000));
        self.run(
            instructions::create_config(
                params.authority,
                params.payer,
                params.stake_pool,
                params.escrow_tree,
                params.data,
            )
            .unwrap(),
            Ok(()),
        );
        self.config = config;
        config
    }

    pub fn config_state(&self) -> Config {
        state::config_from_bytes(&self.account(&self.config).data).unwrap()
    }

    /// Write `config` directly at its PDA and remember the key.
    pub fn put_config(&mut self, config: &Config) -> Address {
        let key = pda::config(&config.stake_pool).0;
        let mut data = vec![0; state::CONFIG_SIZE];
        state::write_config(&mut data, config).unwrap();
        self.accounts.insert(
            key,
            Account {
                lamports: STATE_ACCOUNT_LAMPORTS,
                data,
                owner: ID,
                ..Account::default()
            },
        );
        self.config = key;
        key
    }

    /// Write an epoch page with `header` and no intents at `key`, remember
    /// the key.
    pub fn put_epoch(&mut self, key: Address, header: &EpochHeader) {
        let mut data = vec![0; state::EPOCH_SIZE];
        state::write_epoch(&mut data, header).unwrap();
        self.accounts.insert(
            key,
            Account {
                lamports: STATE_ACCOUNT_LAMPORTS,
                data,
                owner: ID,
                ..Account::default()
            },
        );
        self.epoch = key;
    }

    pub fn epoch_state(&self) -> EpochHeader {
        state::epoch_from_bytes(&self.account(&self.epoch).data).unwrap()
    }

    /// Run `create_epoch` for the current config and remember the page.
    pub fn create_epoch(&mut self, epoch_id: u64, page_index: u64) -> Address {
        self.run(
            instructions::create_epoch(self.config, epoch_id, page_index).unwrap(),
            Ok(()),
        );
        self.epoch = pda::epoch(&self.config, epoch_id, page_index).0;
        self.epoch
    }

    /// Append `count` synthetic escrow hashes (big-endian 1, 2, ...) to the
    /// current epoch page. Exercises the management state machine only.
    pub fn seed_intents(&mut self, count: usize) {
        let hashes: Vec<[u8; 32]> = (1..=count as u64)
            .map(|index| {
                let mut hash = [0; 32];
                if let Some(tail) = hash.get_mut(24..) {
                    tail.copy_from_slice(&index.to_be_bytes());
                }
                hash
            })
            .collect();
        self.seed_intent_hashes(&hashes);
    }

    /// Append escrow UTXO hashes to the current epoch page as `create_intent`
    /// would, collecting the config's intent fee into the epoch (50_000
    /// lamports when no config is stored).
    pub fn seed_intent_hashes(&mut self, hashes: &[[u8; 32]]) {
        let fee = self
            .accounts
            .get(&self.config)
            .and_then(|account| state::config_from_bytes(&account.data).ok())
            .map_or(50_000, |config| config.intent_fee_lamports);
        let epoch = self.accounts.get_mut(&self.epoch).unwrap();
        for hash in hashes {
            state::append_intent(&mut epoch.data, *hash, fee).unwrap();
            epoch.lamports += fee;
        }
    }

    pub fn batch(&self, hash_chain_index: u16) -> Address {
        pda::batch(&self.epoch, hash_chain_index).0
    }

    /// Run `create_batch` on the current config and epoch page.
    pub fn create_batch(
        &mut self,
        payer: Address,
        crank: Address,
        data: CreateBatchIxData,
    ) -> Address {
        let batch = self.batch(data.hash_chain_index);
        self.run(
            instructions::create_batch(payer, crank, self.config, self.epoch, data).unwrap(),
            Ok(()),
        );
        batch
    }

    /// Insert a fabricated stake pool: the borsh `StakePool` account owned by
    /// the stake pool program, a 200-byte `Initialized` reserve stake account
    /// whose staker and withdrawer are the pool withdraw authority, the pool
    /// mint (authority = withdraw authority, supply = `pool_token_supply`,
    /// 9 decimals), and an empty manager fee token account. No validator list
    /// is fabricated; SOL deposits and withdrawals do not read it.
    pub fn stake_pool(&mut self, params: StakePoolParams) -> StakePoolFixture {
        let (withdraw_authority, bump) = spl_stake_pool::find_withdraw_authority_program_address(
            &spl_stake_pool::id(),
            &STAKE_POOL_KEY,
        );
        let pool = StakePool {
            account_type: AccountType::StakePool,
            manager: POOL_MANAGER,
            staker: POOL_MANAGER,
            stake_deposit_authority: spl_stake_pool::find_deposit_authority_program_address(
                &spl_stake_pool::id(),
                &STAKE_POOL_KEY,
            )
            .0,
            stake_withdraw_bump_seed: bump,
            validator_list: VALIDATOR_LIST_KEY,
            reserve_stake: RESERVE_KEY,
            pool_mint: POOL_MINT_KEY,
            manager_fee_account: MANAGER_FEE_KEY,
            token_program_id: TOKEN_PROGRAM,
            total_lamports: params.total_lamports,
            pool_token_supply: params.pool_token_supply,
            last_update_epoch: params.last_update_epoch,
            sol_deposit_fee: params.deposit_fee,
            sol_withdrawal_fee: params.withdrawal_fee,
            sol_referral_fee: 0,
            sol_deposit_authority: None,
            sol_withdraw_authority: None,
            ..StakePool::default()
        };
        let pool_data = borsh::to_vec(&pool).unwrap();
        self.accounts.insert(
            STAKE_POOL_KEY,
            Account {
                lamports: self.rent(pool_data.len()),
                data: pool_data,
                owner: spl_stake_pool::id(),
                ..Account::default()
            },
        );
        let stake_size = StakeStateV2::size_of();
        let reserve_rent = self.rent(stake_size);
        #[allow(deprecated)]
        let reserve_state = StakeStateV2::Initialized(Meta {
            rent_exempt_reserve: reserve_rent,
            authorized: Authorized {
                staker: withdraw_authority,
                withdrawer: withdraw_authority,
            },
            lockup: Lockup::default(),
        });
        let mut reserve_data = borsh::to_vec(&reserve_state).unwrap();
        reserve_data.resize(stake_size, 0);
        self.accounts.insert(
            RESERVE_KEY,
            Account {
                lamports: reserve_rent + params.reserve_extra_lamports,
                data: reserve_data,
                owner: stake_program::ID,
                ..Account::default()
            },
        );
        let mint = TokenMint {
            mint_authority: COption::Some(withdraw_authority),
            supply: params.pool_token_supply,
            decimals: POOL_MINT_DECIMALS,
            is_initialized: true,
            freeze_authority: COption::None,
        };
        let mut mint_data = vec![0; TokenMint::LEN];
        TokenMint::pack(mint, &mut mint_data).unwrap();
        self.accounts.insert(
            POOL_MINT_KEY,
            Account {
                lamports: self.rent(TokenMint::LEN),
                data: mint_data,
                owner: TOKEN_PROGRAM,
                ..Account::default()
            },
        );
        self.token_account(MANAGER_FEE_KEY, POOL_MINT_KEY, POOL_MANAGER, 0);
        StakePoolFixture {
            pool: STAKE_POOL_KEY,
            withdraw_authority,
            reserve: RESERVE_KEY,
            mint: POOL_MINT_KEY,
            manager_fee: MANAGER_FEE_KEY,
            token_program: TOKEN_PROGRAM,
            snapshot: PoolSnapshot {
                total_lamports: params.total_lamports,
                pool_token_supply: params.pool_token_supply,
                deposit_fee: params.deposit_fee,
                withdrawal_fee: params.withdrawal_fee,
            },
        }
    }

    /// Insert an initialized, rent-exempt SPL token account at `key`.
    pub fn token_account(&mut self, key: Address, mint: Address, owner: Address, amount: u64) {
        let token = TokenAccount {
            mint,
            owner,
            amount,
            delegate: COption::None,
            state: AccountState::Initialized,
            is_native: COption::None,
            delegated_amount: 0,
            close_authority: COption::None,
        };
        let mut data = vec![0; TokenAccount::LEN];
        TokenAccount::pack(token, &mut data).unwrap();
        self.accounts.insert(
            key,
            Account {
                lamports: self.rent(TokenAccount::LEN),
                data,
                owner: TOKEN_PROGRAM,
                ..Account::default()
            },
        );
    }

    pub fn token_amount(&self, key: &Address) -> u64 {
        TokenAccount::unpack(&self.account(key).data)
            .unwrap()
            .amount
    }

    pub fn pool_state(&self, pool: &Address) -> StakePool {
        borsh::from_slice(&self.account(pool).data).unwrap()
    }

    /// Insert the config's token vault (the `sol_vault` ATA for the pool
    /// mint) holding `amount` pool tokens.
    pub fn token_vault(&mut self, config_key: Address, config: &Config, amount: u64) -> Address {
        let key = pool::token_vault(&config_key, config);
        self.token_account(key, config.pool_mint, pda::sol_vault(&config_key).0, amount);
        key
    }

    /// Insert the config's system-owned `sol_vault` with its rent-exempt
    /// minimum plus `extra_lamports`.
    pub fn sol_vault(&mut self, config_key: Address, extra_lamports: u64) -> Address {
        let key = pda::sol_vault(&config_key).0;
        let lamports = self.rent(0) + extra_lamports;
        self.accounts.insert(key, wallet(lamports));
        key
    }

    /// Insert the SPP interface accounts a settlement leg moves funds
    /// through: the system-owned SOL interface with its rent-exempt minimum
    /// plus `sol_lamports`, and the SPL interface `[b"spl_asset_vault",
    /// mint]` token account owned by the SPP CPI authority holding
    /// `token_amount`. SPP transact reads no asset registry account.
    pub fn spp_interfaces(
        &mut self,
        pool_mint: Address,
        sol_lamports: u64,
        token_amount: u64,
    ) -> SppInterfaces {
        let lamports = self.rent(0) + sol_lamports;
        self.accounts.insert(SOL_INTERFACE_KEY, wallet(lamports));
        let spl_interface = zolana_interface::pda::spl_interface(&pool_mint);
        self.token_account(spl_interface, pool_mint, SPP_CPI_AUTHORITY, token_amount);
        SppInterfaces {
            sol_interface: SOL_INTERFACE_KEY,
            spl_interface,
        }
    }

    /// Insert the SPP nullifier PDA of `nullifier` in `tree` as queued
    /// (`queue_index` 1), the marker of a cancelled escrow.
    pub fn queued_nullifier(
        &mut self,
        tree: Address,
        nullifier: [u8; 32],
        tree_id: u16,
    ) -> Address {
        let key = zolana_interface::pda::nullifier_pda(&tree, &nullifier).0;
        let mut data = vec![0; NULLIFIER_PDA_SIZE];
        NullifierPda {
            queue_index: 1,
            tree_id,
        }
        .write_to(&mut data)
        .unwrap();
        self.accounts.insert(
            key,
            Account {
                lamports: self.rent(NULLIFIER_PDA_SIZE),
                data,
                owner: SPP,
                ..Account::default()
            },
        );
        key
    }

    /// Insert an escrow tree account with `leaves`.
    pub fn tree(&mut self, tree: Address, leaves: &[[u8; 32]]) -> TreeFixture {
        let fixture = tree_fixture(tree, leaves);
        self.accounts.insert(tree, fixture.account.clone());
        fixture
    }
}
