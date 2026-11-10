//! Mollusk tests of the checks the escrow and withdraw processors run before
//! proof verification. The fixtures carry placeholder proofs, so every check
//! after verification (the SPP account list, the CPI) is out of reach here;
//! the localnet end-to-end test covers those with real proofs.
//!
//! Needs `cargo build-sbf --manifest-path program/Cargo.toml -- --features
//! bpf-entrypoint` first, which writes `target/deploy`.

use mollusk_svm::Mollusk;
use solana_account::Account;
use solana_instruction::{AccountMeta, Instruction};
use solana_program_error::ProgramError;
use solana_pubkey::Pubkey;
use spr_interface::{
    instruction::instruction_data::transact::{
        CircuitId, OwnerTag, TransactIxData, TransactOutput, TransactProof, TreeContext,
    },
    N_PUBLIC_SLOTS, SHIELDED_POOL_PROGRAM_ID,
};
use spr_test_utils::mollusk::expect_err_exact;
use timelock_escrow_program::{
    error::TimelockEscrowError,
    escrow_authority_seeds,
    instructions::{
        escrow::{EscrowIxData, EscrowProof},
        withdraw::{WithdrawIxData, WithdrawProof},
    },
    tag,
};

const SBF_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../target/deploy");

const CREATOR: [u8; 32] = [22; 32];
const PAYER: [u8; 32] = [21; 32];

#[derive(Clone, Copy)]
enum Ix {
    Escrow,
    Withdraw,
}

fn program_id() -> Pubkey {
    Pubkey::new_from_array(*timelock_escrow_program::ID.as_array())
}

fn setup_mollusk() -> Mollusk {
    let (mut mollusk, _) = spr_test_utils::mollusk::mollusk_with_program(
        SBF_DIR,
        *timelock_escrow_program::ID.as_array(),
        "timelock_escrow_program",
    );
    // Past the withdraw fixture's unlock timestamp of 0.
    mollusk.sysvars.clock.unix_timestamp = 1;
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

/// The creator's escrow-authority PDA with `bump`, or `None` if the bump
/// derives an on-curve point.
fn escrow_authority_with_bump(bump: u8) -> Option<Pubkey> {
    let creator = Pubkey::new_from_array(CREATOR);
    let [seed, creator_seed] = escrow_authority_seeds(&creator);
    Pubkey::create_program_address(&[seed, creator_seed, &[bump]], &program_id()).ok()
}

fn canonical_escrow_authority() -> (Pubkey, u8) {
    Pubkey::find_program_address(
        &escrow_authority_seeds(&Pubkey::new_from_array(CREATOR)),
        &program_id(),
    )
}

fn invalid_bump() -> u8 {
    (0..=u8::MAX)
        .rev()
        .find(|bump| escrow_authority_with_bump(*bump).is_none())
        .expect("some bump derives an on-curve point")
}

fn transact() -> TransactIxData {
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
                utxo_hash: [6; 32],
                owner_tag: OwnerTag::Inline([7; 32]),
                data: Some(vec![8; 16]),
            },
        ],
        messages: Vec::new(),
        tree_contexts: vec![TreeContext {
            utxo_tree_root_index: 0,
            nullifier_tree_root_index: 0,
        }],
    }
}

fn ix_data(ix: Ix, authority_bump: u8) -> Vec<u8> {
    let (tag, body) = match ix {
        Ix::Escrow => (
            tag::ESCROW,
            wincode::serialize(&EscrowIxData {
                proof: EscrowProof {
                    proof_a: [10; 32],
                    proof_b: [11; 64],
                    proof_c: [12; 32],
                },
                authority_bump,
                transact: transact(),
            }),
        ),
        Ix::Withdraw => (
            tag::WITHDRAW,
            wincode::serialize(&WithdrawIxData {
                proof: WithdrawProof {
                    proof_a: [10; 32],
                    proof_b: [11; 64],
                    proof_c: [12; 32],
                },
                unlock_timestamp: 0,
                authority_bump,
                transact: transact(),
            }),
        ),
    };
    let mut data = vec![tag];
    data.extend(body.expect("serialize instruction"));
    data
}

/// The signers the processor reads, then a stand-in SPP account list with
/// the canonical escrow-authority PDA.
fn fixture(ix: Ix, authority_bump: u8) -> (Instruction, Vec<(Pubkey, Account)>) {
    let creator = Pubkey::new_from_array(CREATOR);
    let payer = Pubkey::new_from_array(PAYER);
    let tree = Pubkey::new_from_array([23; 32]);
    let spp_id = Pubkey::new_from_array(SHIELDED_POOL_PROGRAM_ID);
    let (escrow_authority, _) = canonical_escrow_authority();

    let mut metas = Vec::new();
    let mut accounts = Vec::new();
    if matches!(ix, Ix::Withdraw) {
        metas.push(AccountMeta::new(payer, true));
        accounts.push((payer, account(1_000_000_000)));
    }
    metas.push(AccountMeta::new_readonly(creator, true));
    accounts.push((creator, account(1_000_000_000)));
    metas.push(AccountMeta::new(payer, true));
    accounts.push((payer, account(1_000_000_000)));
    metas.push(AccountMeta::new(tree, false));
    accounts.push((tree, account(1_000_000_000)));
    metas.push(AccountMeta::new_readonly(escrow_authority, false));
    accounts.push((escrow_authority, account(1_000_000_000)));
    metas.push(AccountMeta::new_readonly(spp_id, false));
    accounts.push((
        spp_id,
        mollusk_svm::program::create_program_account_loader_v3(&spp_id),
    ));

    (
        Instruction {
            program_id: program_id(),
            accounts: metas,
            data: ix_data(ix, authority_bump),
        },
        accounts,
    )
}

#[test]
fn invalid_authority_bump_is_rejected_exactly() {
    let mollusk = setup_mollusk();
    for ix in [Ix::Escrow, Ix::Withdraw] {
        let (instruction, accounts) = fixture(ix, invalid_bump());
        expect_err_exact(
            &mollusk,
            &instruction,
            &accounts,
            ProgramError::Custom(TimelockEscrowError::InvalidEscrowAuthorityBump as u32),
        );
    }
}

#[test]
fn truncated_instruction_data_is_rejected_exactly() {
    let mollusk = setup_mollusk();
    for (ix, ix_tag) in [(Ix::Escrow, tag::ESCROW), (Ix::Withdraw, tag::WITHDRAW)] {
        let (mut instruction, accounts) = fixture(ix, 0);
        instruction.data = vec![ix_tag, 1, 2, 3];
        expect_err_exact(
            &mollusk,
            &instruction,
            &accounts,
            ProgramError::Custom(TimelockEscrowError::InvalidInstructionData as u32),
        );
    }
}

/// The canonical bump passes the derivation and reaches proof verification,
/// which the placeholder proof fails.
#[test]
fn canonical_authority_bump_reaches_proof_verification() {
    let mollusk = setup_mollusk();
    let (_, canonical) = canonical_escrow_authority();
    for ix in [Ix::Escrow, Ix::Withdraw] {
        let (instruction, accounts) = fixture(ix, canonical);
        expect_err_exact(
            &mollusk,
            &instruction,
            &accounts,
            ProgramError::Custom(TimelockEscrowError::ProofVerificationFailed as u32),
        );
    }
}
