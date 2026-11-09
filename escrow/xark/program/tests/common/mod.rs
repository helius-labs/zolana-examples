use mollusk_svm::Mollusk;
use solana_account::Account;
use solana_address::Address;
use solana_instruction::{AccountMeta, Instruction};
use spr_interface::{
    instruction::instruction_data::transact::{
        CircuitId, InputUtxo, OwnerTag, TransactIxData, TransactOutput, TransactProof, TreeContext,
    },
    N_PUBLIC_SLOTS, SHIELDED_POOL_PROGRAM_ID,
};
use timelock_escrow_program::{
    instructions::{
        escrow::{EscrowIxData, EscrowProof},
        withdraw::{WithdrawIxData, WithdrawProof},
    },
    tag, ESCROW_AUTHORITY_PDA_SEED,
};

/// The workspace target: `cargo build-sbf` in `escrow/xark/program` writes
/// the .so to `escrow/xark/target/deploy`.
const SBF_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../target/deploy");

const PAYER: Address = Address::new_from_array([21; 32]);
const CREATOR: Address = Address::new_from_array([22; 32]);
const TREE: Address = Address::new_from_array([23; 32]);
const NULLIFIER_PDA: Address = Address::new_from_array([24; 32]);

/// The unlock timestamp of the withdraw fixture.
pub const UNLOCK_TIMESTAMP: u64 = 1_000;

pub fn setup_mollusk() -> Mollusk {
    let (mollusk, _) = spr_test_utils::mollusk::mollusk_with_program(
        SBF_DIR,
        *timelock_escrow_program::ID.as_array(),
        "timelock_escrow_program",
    );
    mollusk
}

/// A Mollusk whose clock is past [`UNLOCK_TIMESTAMP`].
pub fn setup_unlocked_mollusk() -> Mollusk {
    let mut mollusk = setup_mollusk();
    mollusk.sysvars.clock.unix_timestamp = i64::try_from(UNLOCK_TIMESTAMP).expect("fits i64") + 1;
    mollusk
}

fn account(lamports: u64) -> Account {
    Account {
        lamports,
        data: Vec::new(),
        owner: Address::new_from_array([0; 32]),
        executable: false,
        rent_epoch: 0,
    }
}

fn escrow_authority() -> Address {
    Address::find_program_address(&[ESCROW_AUTHORITY_PDA_SEED], &timelock_escrow_program::ID).0
}

/// A placeholder proof: its points do not decompress, so verification fails.
const PROOF: EscrowProof = EscrowProof {
    a: [10; 32],
    b: [11; 64],
    c: [12; 32],
};

/// A wire-valid 1x2 transact: one input, one output and a compact padding
/// output. Its shape is never checked, because the placeholder proof fails
/// first.
fn spp() -> TransactIxData {
    TransactIxData {
        expiry_unix_ts: u64::MAX,
        private_tx_hash: [1; 32],
        circuit: CircuitId::ConfidentialSigner(1, 2, N_PUBLIC_SLOTS as u8),
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

pub fn escrow_data() -> Vec<u8> {
    let body = wincode::serialize(&EscrowIxData {
        proof: PROOF,
        transact: spp(),
    })
    .expect("serialize escrow");
    let mut data = vec![tag::ESCROW];
    data.extend_from_slice(&body);
    data
}

pub fn withdraw_data(unlock_timestamp: u64) -> Vec<u8> {
    let body = wincode::serialize(&WithdrawIxData {
        proof: WithdrawProof { ..PROOF },
        unlock_timestamp,
        transact: spp(),
    })
    .expect("serialize withdraw");
    let mut data = vec![tag::WITHDRAW];
    data.extend_from_slice(&body);
    data
}

/// Applies `edit` to every meta of the payer, which is the withdraw caller
/// (account 0) and the SPP payer (account 2): a privilege kept on one would
/// hold for both.
pub fn edit_payer(instruction: &mut Instruction, edit: impl Fn(&mut AccountMeta)) {
    instruction
        .accounts
        .iter_mut()
        .filter(|meta| meta.pubkey == PAYER)
        .for_each(edit);
}

/// An escrow instruction with the builder's account layout: the creator,
/// then the forwarded SPP list ending in `escrow_authority`.
pub fn escrow_fixture() -> (Instruction, Vec<(Address, Account)>) {
    fixture(vec![AccountMeta::new(CREATOR, true)], escrow_data())
}

/// A withdraw instruction at [`UNLOCK_TIMESTAMP`] with the builder's account
/// layout: the caller, the creator, then the forwarded SPP list ending in
/// `escrow_authority`.
pub fn withdraw_fixture() -> (Instruction, Vec<(Address, Account)>) {
    fixture(
        vec![
            AccountMeta::new(PAYER, true),
            AccountMeta::new_readonly(CREATOR, true),
        ],
        withdraw_data(UNLOCK_TIMESTAMP),
    )
}

/// An instruction on `data` with `program_accounts` ahead of the forwarded
/// SPP accounts, which end with the escrow-authority PDA.
fn fixture(
    program_accounts: Vec<AccountMeta>,
    data: Vec<u8>,
) -> (Instruction, Vec<(Address, Account)>) {
    let spp_id = Address::new_from_array(SHIELDED_POOL_PROGRAM_ID);
    let escrow_authority = escrow_authority();

    let mut metas = program_accounts;
    metas.extend([
        AccountMeta::new(PAYER, true),
        AccountMeta::new(TREE, false),
        AccountMeta::new_readonly(spp_id, false),
        AccountMeta::new_readonly(Address::default(), false),
        AccountMeta::new(TREE, false),
        AccountMeta::new(NULLIFIER_PDA, false),
        AccountMeta::new_readonly(escrow_authority, false),
    ]);
    let accounts = vec![
        (PAYER, account(1_000_000_000)),
        (CREATOR, account(1_000_000_000)),
        (TREE, account(1_000_000_000)),
        (
            spp_id,
            mollusk_svm::program::create_program_account_loader_v3(&spp_id),
        ),
        (Address::default(), account(1_000_000_000)),
        (NULLIFIER_PDA, account(0)),
        (escrow_authority, account(0)),
    ];

    (
        Instruction {
            program_id: timelock_escrow_program::ID,
            accounts: metas,
            data,
        },
        accounts,
    )
}
