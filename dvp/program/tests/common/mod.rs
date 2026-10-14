use dvp_program::{
    instructions::{
        create_dvp::{CreateDvpIxData, CreateDvpProof},
        reclaim_dvp::{ReclaimDvpIxData, ReclaimDvpProof},
        settle_dvp::{SettleDvpIxData, SettleDvpProof},
    },
    tag, DVP_AUTHORITY_PDA_SEED,
};
use mollusk_svm::Mollusk;
use solana_account::Account;
use solana_instruction::{AccountMeta, Instruction};
use solana_pubkey::Pubkey;
use zolana_interface::{
    instruction::instruction_data::transact::{
        CircuitId, InputUtxo, OwnerTag, TransactIxData, TransactOutput, TransactProof, TreeContext,
    },
    N_PUBLIC_SLOTS, SHIELDED_POOL_PROGRAM_ID,
};

/// The workspace target: `cargo build-sbf` in `dvp/program` writes the .so
/// to `dvp/target/deploy`.
const SBF_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../target/deploy");

const PAYER: Pubkey = Pubkey::new_from_array([21; 32]);
pub const CREATOR: Pubkey = Pubkey::new_from_array([22; 32]);
const TREE: Pubkey = Pubkey::new_from_array([23; 32]);
const NULLIFIER_PDA: Pubkey = Pubkey::new_from_array([24; 32]);
/// The settlement authority.
const SETTLEMENT_AUTHORITY: Pubkey = Pubkey::new_from_array([26; 32]);
/// The reclaiming party.
const SIGNER: Pubkey = Pubkey::new_from_array([25; 32]);
/// A field element (below the BN254 modulus).
pub const DVP_ID: [u8; 32] = [1; 32];

pub fn setup_mollusk() -> Mollusk {
    let (mollusk, _) = zolana_test_utils::mollusk::mollusk_with_program(
        SBF_DIR,
        *dvp_program::ID.as_array(),
        "dvp_program",
    );
    mollusk
}

fn account(lamports: u64) -> Account {
    Account {
        lamports,
        data: Vec::new(),
        owner: Pubkey::new_from_array([0; 32]),
        executable: false,
        rent_epoch: 0,
    }
}

pub fn program_id() -> Pubkey {
    Pubkey::new_from_array(*dvp_program::ID.as_array())
}

/// The `dvp_authority` PDA and its bump for `creator` and `dvp_id`.
pub fn dvp_authority(creator: &Pubkey, dvp_id: &[u8; 32]) -> (Pubkey, u8) {
    Pubkey::find_program_address(
        &[DVP_AUTHORITY_PDA_SEED, creator.as_ref(), dvp_id],
        &program_id(),
    )
}

/// A wire-valid 1x2 transact: one input slot (the create address or the
/// reclaimed deposit), one output (the DvP UTXO or the refund) and a compact
/// padding output.
fn spp() -> TransactIxData {
    TransactIxData {
        expiry_unix_ts: u64::MAX,
        private_tx_hash: [1; 32],
        circuit: CircuitId::ConfidentialEddsa(1, 2, N_PUBLIC_SLOTS as u8),
        tx_viewing_pk: [2; 33],
        salt: [3; 16],
        proof: TransactProof::zeroed(),
        inputs: vec![InputUtxo {
            nullifier_hash: [6; 32],
            tree_index: 0,
        }],
        interface_transfers: Vec::new(),
        data_hash: None,
        ring_data_hash: None,
        outputs: vec![TransactOutput {
            utxo_hash: [4; 32],
            owner_tag: OwnerTag::Inline([5; 32]),
            data: None,
        }],
        messages: Vec::new(),
        tree_contexts: vec![TreeContext {
            utxo_tree_root_index: 0,
            nullifier_tree_root_index: 0,
        }],
    }
}

pub fn create_dvp_data(dvp_id: [u8; 32], bump: u8) -> Vec<u8> {
    let body = wincode::serialize(&CreateDvpIxData {
        dvp_id,
        bump,
        create_proof: CreateDvpProof {
            proof_a: [10; 32],
            proof_b: [11; 64],
            proof_c: [12; 32],
        },
        spp: spp(),
    })
    .expect("serialize create_dvp");
    let mut data = vec![tag::CREATE_DVP];
    data.extend_from_slice(&body);
    data
}

/// A bump whose `(CREATOR, DVP_ID)` seeds hash onto the curve, so it has no
/// PDA.
pub fn off_curve_bump() -> u8 {
    (0..=u8::MAX)
        .find(|bump| {
            Pubkey::create_program_address(
                &[DVP_AUTHORITY_PDA_SEED, CREATOR.as_ref(), &DVP_ID, &[*bump]],
                &program_id(),
            )
            .is_err()
        })
        .expect("some bump lands on the curve")
}

pub fn reclaim_dvp_data(creator: Pubkey, dvp_id: [u8; 32], bump: u8) -> Vec<u8> {
    let body = wincode::serialize(&ReclaimDvpIxData {
        creator,
        dvp_id,
        bump,
        reclaim_proof: ReclaimDvpProof {
            proof_a: [10; 32],
            proof_b: [11; 64],
            proof_c: [12; 32],
        },
        spp: spp(),
    })
    .expect("serialize reclaim_dvp");
    let mut data = vec![tag::RECLAIM_DVP];
    data.extend_from_slice(&body);
    data
}

/// The clock the settle_dvp failing tests warp Mollusk to; the settle
/// fixture's `current_timestamp` equals it.
pub const SETTLE_NOW: i64 = 1_000;

/// Settle data on the 1x2 placeholder transact: its shape is never checked,
/// because the placeholder proof fails first.
pub fn settle_dvp_data(
    creator: Pubkey,
    dvp_id: [u8; 32],
    bump: u8,
    current_timestamp: i64,
) -> Vec<u8> {
    let body = wincode::serialize(&SettleDvpIxData {
        creator,
        dvp_id,
        bump,
        current_timestamp,
        settle_proof: SettleDvpProof {
            proof_a: [10; 32],
            proof_b: [11; 64],
            proof_c: [12; 32],
        },
        spp: spp(),
    })
    .expect("serialize settle_dvp");
    let mut data = vec![tag::SETTLE_DVP];
    data.extend_from_slice(&body);
    data
}

/// A create_dvp instruction with the builder's account layout: payer,
/// creator, then the forwarded SPP list ending in `dvp_authority` (D8).
pub fn create_dvp_fixture() -> (Instruction, Vec<(Pubkey, Account)>) {
    let (_, bump) = dvp_authority(&CREATOR, &DVP_ID);
    fixture(CREATOR, create_dvp_data(DVP_ID, bump))
}

/// A reclaim_dvp instruction by `SIGNER` with the builder's account layout:
/// payer, signer, then the forwarded SPP list ending in `dvp_authority`.
pub fn reclaim_dvp_fixture() -> (Instruction, Vec<(Pubkey, Account)>) {
    let (_, bump) = dvp_authority(&CREATOR, &DVP_ID);
    fixture(SIGNER, reclaim_dvp_data(CREATOR, DVP_ID, bump))
}

/// A settle_dvp instruction by `SETTLEMENT_AUTHORITY` at `SETTLE_NOW`, with
/// the builder's account layout: payer, settlement authority, then the
/// forwarded SPP list ending in `dvp_authority`.
pub fn settle_dvp_fixture() -> (Instruction, Vec<(Pubkey, Account)>) {
    let (_, bump) = dvp_authority(&CREATOR, &DVP_ID);
    fixture(
        SETTLEMENT_AUTHORITY,
        settle_dvp_data(CREATOR, DVP_ID, bump, SETTLE_NOW),
    )
}

/// An instruction on `data` with the role signer `role` after the payer,
/// and the `(CREATOR, DVP_ID)` PDA as the last forwarded SPP account.
fn fixture(role: Pubkey, data: Vec<u8>) -> (Instruction, Vec<(Pubkey, Account)>) {
    let spp_id = Pubkey::new_from_array(SHIELDED_POOL_PROGRAM_ID);
    let (dvp_authority, _) = dvp_authority(&CREATOR, &DVP_ID);

    let metas = vec![
        AccountMeta::new(PAYER, true),
        AccountMeta::new_readonly(role, true),
        AccountMeta::new(PAYER, true),
        AccountMeta::new(TREE, false),
        AccountMeta::new_readonly(spp_id, false),
        AccountMeta::new_readonly(Pubkey::default(), false),
        AccountMeta::new(TREE, false),
        AccountMeta::new(NULLIFIER_PDA, false),
        AccountMeta::new_readonly(dvp_authority, false),
    ];
    let accounts = vec![
        (PAYER, account(1_000_000_000)),
        (role, account(1_000_000_000)),
        (TREE, account(1_000_000_000)),
        (
            spp_id,
            mollusk_svm::program::create_program_account_loader_v3(&spp_id),
        ),
        (Pubkey::default(), account(1_000_000_000)),
        (NULLIFIER_PDA, account(0)),
        (dvp_authority, account(0)),
    ];

    (
        Instruction {
            program_id: program_id(),
            accounts: metas,
            data,
        },
        accounts,
    )
}
