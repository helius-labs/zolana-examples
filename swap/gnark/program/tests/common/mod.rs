use std::sync::Once;

use mollusk_svm::{result::ProgramResult, Mollusk};
use solana_account::Account;
use solana_instruction::{AccountMeta, Instruction};
use solana_program_error::ProgramError;
use solana_pubkey::Pubkey;
use spr_interface::{
    instruction::instruction_data::transact::{
        CircuitId, MessageData, OwnerTag, TransactIxData, TransactOutput, TransactProof,
        TreeContext,
    },
    N_PUBLIC_SLOTS, SHIELDED_POOL_PROGRAM_ID,
};
use swap_program::{
    instructions::{
        cancel::{CancelIxData, CancelProof},
        make::{MakeIxData, MakeProof, MarkerData},
        take::{TakeIxData, TakeProof},
    },
    order_authority_seeds, tag,
};

pub const MAKER: [u8; 32] = [22; 32];
/// The fee payer, which is also the maker of a make.
pub const PAYER: [u8; 32] = [21; 32];
/// The order output's hash in [`transact`].
pub const ORDER_UTXO_HASH: [u8; 32] = [6; 32];

const SBF_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/target/deploy");

#[derive(Clone, Copy, Debug)]
pub enum Wrapper {
    Make,
    Take,
    Cancel,
}

impl Wrapper {
    pub fn tag(self) -> u8 {
        match self {
            Self::Make => tag::MAKE,
            Self::Take => tag::TAKE,
            Self::Cancel => tag::CANCEL,
        }
    }

    fn signed(self) -> bool {
        !matches!(self, Self::Make)
    }

    fn has_maker(self) -> bool {
        matches!(self, Self::Cancel)
    }

    /// The maker whose order-authority PDA the instruction derives: the payer
    /// of a make, else [`MAKER`].
    pub fn maker(self) -> [u8; 32] {
        match self {
            Self::Make => PAYER,
            Self::Take | Self::Cancel => MAKER,
        }
    }
}

fn swap_id() -> Pubkey {
    Pubkey::new_from_array(*swap_program::ID.as_array())
}

/// `maker`'s order-authority PDA with `bump`, or `None` if the bump derives
/// an on-curve point.
pub fn order_authority_with_bump(maker: [u8; 32], bump: u8) -> Option<Pubkey> {
    let maker = Pubkey::new_from_array(maker);
    let [seed, maker_seed] = order_authority_seeds(&maker);
    Pubkey::create_program_address(&[seed, maker_seed, &[bump]], &swap_id()).ok()
}

/// `maker`'s order-authority PDA and its canonical bump.
pub fn canonical_order_authority(maker: [u8; 32]) -> (Pubkey, u8) {
    Pubkey::find_program_address(
        &order_authority_seeds(&Pubkey::new_from_array(maker)),
        &swap_id(),
    )
}

/// A bump that derives no order-authority PDA for `maker`.
pub fn invalid_bump(maker: [u8; 32]) -> u8 {
    (0..=u8::MAX)
        .rev()
        .find(|bump| order_authority_with_bump(maker, *bump).is_none())
        .expect("some bump derives an on-curve point")
}

static SETUP: Once = Once::new();

/// A Mollusk instance with the swap program loaded from [`SBF_DIR`]. The first
/// call in a process sets `SBF_OUT_DIR` and quiets `RUST_LOG` unless the
/// caller set it.
pub fn setup_mollusk() -> (Mollusk, Pubkey) {
    SETUP.call_once(|| {
        std::env::set_var("SBF_OUT_DIR", SBF_DIR);
        if std::env::var_os("RUST_LOG").is_none() {
            std::env::set_var("RUST_LOG", "error");
        }
    });
    let program_id = swap_id();
    let mut mollusk = Mollusk::default();
    mollusk.add_program(&program_id, "swap_program");
    (mollusk, program_id)
}

/// Assert the instruction fails with exactly `expected`.
#[track_caller]
pub fn expect_err_exact(
    mollusk: &Mollusk,
    instruction: &Instruction,
    accounts: &[(Pubkey, Account)],
    expected: ProgramError,
) {
    let result = mollusk.process_instruction(instruction, accounts);
    assert_eq!(
        result.program_result,
        ProgramResult::Failure(expected),
        "unexpected program result"
    );
}

pub fn account(lamports: u64) -> Account {
    Account {
        lamports,
        data: Vec::new(),
        owner: Pubkey::new_from_array([0; 32]),
        executable: false,
        rent_epoch: 0,
    }
}

pub fn transact(messages: Vec<MessageData>) -> TransactIxData {
    TransactIxData {
        expiry_unix_ts: u64::MAX,
        private_tx_hash: [1; 32],
        circuit: CircuitId::ConfidentialSigner(1, 2, N_PUBLIC_SLOTS as u8),
        tx_viewing_pk: [2; 33],
        salt: [3; 16],
        proof: TransactProof::zeroed(),
        inputs: Vec::new(),
        interface_transfers: Vec::new(),
        data_hash: None,
        ring_data_hash: None,
        outputs: vec![
            TransactOutput {
                utxo_hash: [4; 32],
                owner_tag: OwnerTag::Inline([5; 32]),
                data: None,
            },
            TransactOutput {
                utxo_hash: ORDER_UTXO_HASH,
                owner_tag: OwnerTag::Inline([7; 32]),
                data: Some(vec![8; 16]),
            },
        ],
        messages,
        tree_contexts: vec![TreeContext {
            utxo_tree_root_index: 0,
            nullifier_tree_root_index: 0,
        }],
    }
}

/// The marker the program requires: the order output's hash and the maker.
pub fn valid_marker_data() -> Vec<u8> {
    borsh::to_vec(&MarkerData {
        order_utxo_hash: ORDER_UTXO_HASH,
        maker_pubkey: PAYER,
    })
    .expect("serialize marker")
}

pub fn marker(data: Vec<u8>) -> MessageData {
    MessageData {
        view_tag: [9; 32],
        data,
    }
}

/// The fixture's transact data: a make carries the valid marker.
pub fn fixture_transact(wrapper: Wrapper) -> TransactIxData {
    transact(if matches!(wrapper, Wrapper::Make) {
        vec![marker(valid_marker_data())]
    } else {
        Vec::new()
    })
}

pub fn wrapper_data(wrapper: Wrapper) -> Vec<u8> {
    wrapper_data_with(wrapper, fixture_transact(wrapper))
}

/// Serialize a wrapper body around a caller-customized transact payload, so
/// negatives can tamper with individual transact fields while staying
/// wire-valid. It carries the maker's canonical order-authority bump.
pub fn wrapper_data_with(wrapper: Wrapper, transact: TransactIxData) -> Vec<u8> {
    let (_, bump) = canonical_order_authority(wrapper.maker());
    wrapper_data_with_bump(wrapper, transact, bump)
}

/// [`wrapper_data`] with the order-authority bump `authority_bump`.
pub fn wrapper_data_with_bump(
    wrapper: Wrapper,
    transact: TransactIxData,
    authority_bump: u8,
) -> Vec<u8> {
    let body = match wrapper {
        Wrapper::Make => wincode::serialize(&MakeIxData {
            proof: MakeProof {
                proof_a: [10; 32],
                proof_b: [11; 64],
                proof_c: [12; 32],
            },
            authority_bump,
            transact,
        }),
        Wrapper::Take => wincode::serialize(&TakeIxData {
            proof: TakeProof {
                proof_a: [10; 32],
                proof_b: [11; 64],
                proof_c: [12; 32],
            },
            maker: MAKER,
            authority_bump,
            transact,
        }),
        Wrapper::Cancel => wincode::serialize(&CancelIxData {
            proof: CancelProof {
                proof_a: [10; 32],
                proof_b: [11; 64],
                proof_c: [12; 32],
            },
            order_expiry: 0,
            authority_bump,
            transact,
        }),
    }
    .expect("serialize wrapper");
    let mut data = vec![wrapper.tag()];
    data.extend_from_slice(&body);
    data
}

pub fn fixture(wrapper: Wrapper) -> (Instruction, Vec<(Pubkey, Account)>) {
    let payer = Pubkey::new_from_array(PAYER);
    let maker = Pubkey::new_from_array(MAKER);
    let tree = Pubkey::new_from_array([23; 32]);
    let spp_id = Pubkey::new_from_array(SHIELDED_POOL_PROGRAM_ID);
    let swap_id = swap_id();
    let (order_authority, _) = canonical_order_authority(wrapper.maker());

    let mut metas = vec![AccountMeta::new(payer, true)];
    let mut accounts = vec![(payer, account(1_000_000_000))];
    if wrapper.has_maker() {
        metas.push(AccountMeta::new_readonly(maker, true));
        accounts.push((maker, account(1_000_000_000)));
    }
    metas.push(AccountMeta::new(payer, true));
    accounts.push((payer, account(1_000_000_000)));
    metas.push(AccountMeta::new(tree, false));
    accounts.push((tree, account(1_000_000_000)));
    if wrapper.signed() {
        metas.push(AccountMeta::new_readonly(order_authority, false));
        accounts.push((order_authority, account(1_000_000_000)));
    }
    metas.push(AccountMeta::new_readonly(spp_id, false));
    accounts.push((
        spp_id,
        mollusk_svm::program::create_program_account_loader_v3(&spp_id),
    ));

    (
        Instruction {
            program_id: swap_id,
            accounts: metas,
            data: wrapper_data(wrapper),
        },
        accounts,
    )
}
