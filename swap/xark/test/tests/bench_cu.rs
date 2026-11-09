//! Benchmark of the swap make, take and cancel instructions: compute units
//! replayed under mollusk, warm proving times, key loading, circuit and
//! transaction sizes. Writes `BENCHMARK.md` in the format shared by every
//! gnark and Xark example.
//!
//! The profiled swap program calls a profiler syscall that a validator does
//! not register, so it is built into its own directory, never
//! `target/deploy` (the e2e tests deploy from there). From `swap/xark/`:
//!
//! ```sh
//! cargo build-sbf --manifest-path program/Cargo.toml --tools-version v1.54 \
//!     --sbf-out-dir target/bench -- --locked --features bpf-entrypoint,profile-program
//! SHIELDED_POOL_PROGRAM_PATH=/path/to/shielded_pool_program.so \
//! ZOLANA_CLI_BIN=/path/to/zolana \
//!     cargo test --release --manifest-path test/Cargo.toml --test bench_cu -- --ignored --nocapture
//! ```
//!
//! - The shielded pool program is read from `SHIELDED_POOL_PROGRAM_PATH`, else
//!   from `target/bench/shielded_pool_program.so`.
//! - The SPP proofs come from the local prover server: one already serving
//!   `ZOLANA_PROVER_URL` is reused, else the `zolana` CLI (`ZOLANA_CLI_BIN`)
//!   starts one with the keys in `ZOLANA_PROVER_KEYS_DIR`, else
//!   `~/.config/zolana/proving-keys`.
//! - The swap circuit proofs come from the checksum-pinned Xark artifacts in
//!   `target/zk/swap-program`. Each cold key load runs in a child process of
//!   the bench (`key_load_probe`).
#[path = "../../../../test-utils/bench_report.rs"]
mod bench_report;

use std::{
    env, fs,
    path::{Path, PathBuf},
    time::Duration,
};

use bench_report::{
    cold_key_load, proof_bytes, time_key_load, warm_median, CircuitRow, Implementation,
    InstructionRow, Report,
};
use light_program_profiler::mollusk::{register_profiling_syscalls, take_profiling_entries};
use mollusk_svm::{program::loader_keys::LOADER_V3, result::Check, Mollusk};
use num_bigint::BigUint;
use solana_account::Account;
use solana_address::Address;
use solana_instruction::{AccountMeta, Instruction};
use solana_keypair::Keypair;
use solana_pubkey::Pubkey;
use solana_signer::Signer;
use spr_client::{
    transaction_size, ComputeBudgetConfig, MerkleContext, MerkleProof, NonInclusionProof,
    ProverClient, ProverExt, SpendProof, NULLIFIER_TREE_HEIGHT, STATE_TREE_HEIGHT,
};
use spr_hasher::Poseidon;
use spr_interface::{
    instruction::instruction_data::transact::TransactIxData,
    state::{
        default_tree_fees, discriminator::TREE_ACCOUNT_DISCRIMINATOR, nullifier_tree_params,
        tree_account_size, STATE_HEIGHT,
    },
    SHIELDED_POOL_PROGRAM_ID,
};
use spr_keypair::{random_blinding, NullifierKey, ShieldedKeypair, SigningKey};
use spr_merkle_tree::{indexed::IndexedMerkleTree, MerkleTree};
use spr_program::{proof::compressed_proof, CompressedProof, ProgramOwner, ProgramProofInputs};
use spr_transaction::{
    instructions::transact::{SppProofInputs, BN254_MODULUS_DEC},
    Data, Mint, Utxo, WalletUtxo,
};
use spr_tree::TreeAccount;
use swap_sdk::{
    instructions::{
        cancel::{Cancel, CancelTransactionParams},
        make::{Make, MakeTransactionParams},
        take::{Take, TakeTransactionParams},
    },
    proof::{checked_circuit, constraint_count, keys_dir, verifying_key, CircuitId},
    shared::identity,
    state::{OrderTerms, OrderUtxo},
};

const PROFILING_SBF_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../target/bench");
const OUTPUT_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../BENCHMARK.md");

/// Raw id of the fixture tree. `build_tree_fixture` initializes the account with
/// this id, and every UTXO commitment folds it in.
const BENCH_TREE_ID: u16 = 0;
/// The runtime slot, after the slots `build_tree_fixture` appends leaves at.
const BENCH_SLOT: u64 = 1_000;
const SOURCE_AMOUNT: u64 = 400_000;
const DESTINATION_AMOUNT: u64 = 250;
const EXPIRY: u64 = 1_900_000_000;

#[test]
#[ignore = "benchmark; slow, needs the profiling SBF build and a prover. See the module docs"]
fn bench_cu_swap() {
    env::set_var("SBF_OUT_DIR", PROFILING_SBF_DIR);

    let swap_id = Pubkey::new_from_array(*swap_program::ID.as_array());
    let spp_id = Pubkey::new_from_array(SHIELDED_POOL_PROGRAM_ID);

    let mut mollusk = Mollusk::default();
    // Keep the runtime slot after every fixture append: SPP refuses a tree
    // whose last update is ahead of the clock.
    mollusk.warp_to_slot(BENCH_SLOT);
    register_profiling_syscalls(&mut mollusk);
    mollusk.add_program(&swap_id, "swap_program");
    mollusk.add_program_with_loader_and_elf(&spp_id, &LOADER_V3, &shielded_pool_elf());

    start_prover();
    let prover = ProverClient::local();
    let (circuits, instructions) = [
        bench_make(&mut mollusk, &spp_id, &prover),
        bench_take(&mut mollusk, &spp_id, &prover),
        bench_cancel(&mut mollusk, &spp_id, &prover),
    ]
    .into_iter()
    .unzip();
    Report {
        example: "Confidential Swap",
        implementation: Implementation::Xark,
        circuits,
        instructions,
    }
    .write(OUTPUT_PATH);
}

/// The child-process half of the cold key load: one load of the checked
/// bytecode and proving key of the circuit `cold_key_load` names.
#[test]
#[ignore = "child process of bench_cu_swap"]
fn key_load_probe() {
    time_key_load(|name| {
        let circuit = CircuitId::ALL
            .into_iter()
            .find(|circuit| circuit.name() == name)
            .expect("known circuit");
        checked_circuit(circuit).expect("load checked circuit")
    });
}

fn bench_make(
    mollusk: &mut Mollusk,
    spp_id: &Pubkey,
    prover: &ProverClient,
) -> (CircuitRow, InstructionRow) {
    let tree = spr_interface::pda::tree(BENCH_TREE_ID);
    let payer = Keypair::new();
    let maker = keypair_from_payer(&payer);
    let taker = keypair_from_payer(&Keypair::new());
    let taker_address = taker.shielded_address().expect("taker address");

    // Like the localnet flow, the maker funds the order from its own note.
    let source = wallet_utxo(&maker, 1_000_000, 0);
    let transaction = MakeTransactionParams {
        terms: terms(&maker, Some(&taker)),
        source_amount: SOURCE_AMOUNT,
        source,
        taker_address,
        payer: payer.pubkey(),
    }
    .build(&maker)
    .expect("make transaction");

    let (tree_account, spend_proofs) = tree_fixture(&tree, &transaction.spp_proof_inputs);
    let (spp_proof, spp_time) = prove_spp(
        transaction.spp_proof_inputs,
        &spend_proofs,
        prover,
        &[&maker.nullifier_key],
    );
    let (make_proof, circuit) = measure_circuit(CircuitId::Make, &transaction.proof_inputs);
    let ix = Make {
        payer: payer.pubkey(),
        tree,
        make_proof,
        spp_proof,
    }
    .instruction()
    .expect("make instruction");
    replay(
        mollusk,
        spp_id,
        "make",
        circuit,
        &ix,
        &payer.pubkey(),
        tree_account,
        spp_time,
    )
}

fn bench_take(
    mollusk: &mut Mollusk,
    spp_id: &Pubkey,
    prover: &ProverClient,
) -> (CircuitRow, InstructionRow) {
    let tree = spr_interface::pda::tree(BENCH_TREE_ID);
    let taker_payer = Keypair::new();
    let taker = keypair_from_payer(&taker_payer);
    let maker = keypair_from_payer(&Keypair::new());

    let order_utxo = OrderUtxo {
        terms: terms(&maker, Some(&taker)),
        blinding: random_blinding(),
        source_mint: Mint::SOL,
        source_amount: SOURCE_AMOUNT,
    };
    let taker_input = wallet_utxo(&taker, DESTINATION_AMOUNT, 1);
    let transaction = TakeTransactionParams {
        order_utxo,
        order_leaf_index: 0,
        taker_input,
        payer: taker_payer.pubkey(),
        tree_id: BENCH_TREE_ID,
    }
    .build(&taker)
    .expect("take transaction");

    let (tree_account, spend_proofs) = tree_fixture(&tree, &transaction.spp_proof_inputs);
    let (spp_proof, spp_time) = prove_spp(
        transaction.spp_proof_inputs,
        &spend_proofs,
        prover,
        &[&ProgramOwner::nullifier_key(), &taker.nullifier_key],
    );
    let (take_proof, circuit) = measure_circuit(CircuitId::Take, &transaction.proof_inputs);
    let ix = Take {
        payer: taker_payer.pubkey(),
        tree,
        take_proof,
        spp_proof,
    }
    .instruction()
    .expect("take instruction");
    replay(
        mollusk,
        spp_id,
        "take",
        circuit,
        &ix,
        &taker_payer.pubkey(),
        tree_account,
        spp_time,
    )
}

fn bench_cancel(
    mollusk: &mut Mollusk,
    spp_id: &Pubkey,
    prover: &ProverClient,
) -> (CircuitRow, InstructionRow) {
    const ORDER_EXPIRY: u64 = 1_000_000;

    let tree = spr_interface::pda::tree(BENCH_TREE_ID);
    let maker_payer = Keypair::new();
    let maker = keypair_from_payer(&maker_payer);
    let taker = keypair_from_payer(&Keypair::new());

    let order_utxo = OrderUtxo {
        terms: OrderTerms {
            expiry: ORDER_EXPIRY,
            ..terms(&maker, Some(&taker))
        },
        blinding: random_blinding(),
        source_mint: Mint::SOL,
        source_amount: SOURCE_AMOUNT,
    };
    let transaction = CancelTransactionParams {
        order_utxo,
        order_leaf_index: 0,
        payer: maker_payer.pubkey(),
        tree_id: BENCH_TREE_ID,
    }
    .build(&maker)
    .expect("cancel transaction");

    let (tree_account, spend_proofs) = tree_fixture(&tree, &transaction.spp_proof_inputs);
    let (spp_proof, spp_time) = prove_spp(
        transaction.spp_proof_inputs.clone(),
        &spend_proofs,
        prover,
        &[&ProgramOwner::nullifier_key()],
    );
    let (cancel_proof, circuit) = measure_circuit(CircuitId::Cancel, &transaction.proof_inputs);
    let ix = Cancel {
        maker: maker_payer.pubkey(),
        payer: maker_payer.pubkey(),
        tree,
        cancel_proof,
        order_expiry: ORDER_EXPIRY,
        spp_proof,
    }
    .instruction()
    .expect("cancel instruction");
    mollusk.sysvars.clock.unix_timestamp = ORDER_EXPIRY as i64 + 1;
    replay(
        mollusk,
        spp_id,
        "cancel",
        circuit,
        &ix,
        &maker_payer.pubkey(),
        tree_account,
        spp_time,
    )
}

/// Order terms of `maker`, designated to `taker`, paid in SOL.
fn terms(maker: &ShieldedKeypair, taker: Option<&ShieldedKeypair>) -> OrderTerms {
    OrderTerms {
        destination_mint: Mint::SOL,
        destination_amount: DESTINATION_AMOUNT,
        maker: maker.shielded_address().expect("maker address"),
        expiry: EXPIRY,
        taker: taker.map(|taker| {
            identity(
                &taker
                    .shielded_address()
                    .expect("taker address")
                    .solana_address()
                    .expect("taker solana address"),
            )
            .expect("taker identity")
        }),
    }
}

/// `amount` lamports owned by `owner` at `leaf_index` of the fixture tree.
fn wallet_utxo(owner: &ShieldedKeypair, amount: u64, leaf_index: u64) -> WalletUtxo {
    spr_test_utils::utxo::wallet(
        Utxo {
            owner: owner.signing_pubkey(),
            asset: Mint::SOL,
            amount,
            blinding: random_blinding(),
            ring_program_id: None,
            data: Data::default(),
        },
        &owner.nullifier_key,
        BENCH_TREE_ID,
        leaf_index,
        None,
        None,
    )
    .expect("wallet utxo")
}

/// Replays `ix` under mollusk and collects its profile and sizes.
#[allow(clippy::too_many_arguments)]
fn replay(
    mollusk: &mut Mollusk,
    spp_id: &Pubkey,
    name: &str,
    circuit: CircuitRow,
    ix: &Instruction,
    payer: &Pubkey,
    tree_account: Account,
    warm_spp_proof: Duration,
) -> (CircuitRow, InstructionRow) {
    let fixtures = vec![
        (spr_interface::pda::tree(BENCH_TREE_ID), tree_account),
        (*payer, system_owned_account(100_000_000_000)),
    ];
    let accounts = assemble_accounts(ix, spp_id, &fixtures);
    mollusk.process_and_validate_instruction(
        &to_mollusk_instruction(ix),
        &accounts,
        &[Check::success()],
    );
    let entries = take_profiling_entries();
    assert!(!entries.is_empty(), "no profiling entries for '{name}'");
    let v1_tx_bytes = transaction_size(
        payer,
        std::slice::from_ref(ix),
        ComputeBudgetConfig::new(1_400_000),
    )
    .expect("measure v1 transaction")
    .bytes;
    let instruction = InstructionRow {
        name: name.into(),
        circuit: circuit.name.clone(),
        entries,
        warm_spp_proof,
        instruction_data_bytes: u64::try_from(ix.data.len()).expect("fits u64"),
        accounts: u64::try_from(ix.accounts.len()).expect("fits u64"),
        v1_tx_bytes: u64::try_from(v1_tx_bytes).expect("fits u64"),
    };
    (circuit, instruction)
}

/// Proves the SPP transaction against the running prover server.
fn prove_spp(
    proof_inputs: SppProofInputs,
    spend_proofs: &[SpendProof],
    prover: &ProverClient,
    keys: &[&NullifierKey],
) -> (TransactIxData, Duration) {
    let dummy_proofs = spr_test_utils::utxo::dummy_proofs(&proof_inputs);
    warm_median(|| {
        prover
            .prove_transact(
                proof_inputs.clone(),
                spend_proofs,
                &dummy_proofs,
                &spr_test_utils::utxo::ProofKeys(keys),
            )
            .expect("prove transact")
    })
}

/// Measure `id` on `inputs`: its cold key loads, its warm proof time from one
/// loaded checked circuit copy, and its sizes. The last proof, checked
/// against the verifying key, is returned for the instruction.
fn measure_circuit(id: CircuitId, inputs: &ProgramProofInputs) -> (CompressedProof, CircuitRow) {
    let (key_load, _) = cold_key_load(id.name());
    let (_private, circuit) = checked_circuit(id).expect("checked circuit");
    let (proven, warm_proof) = warm_median(|| circuit.prove(inputs.clone()).expect("swap proof"));
    let proof = compressed_proof(&proven.proof).expect("compressed proof");
    let verifying_key = verifying_key(id).expect("verifying key");
    let public_hash = spr_program::from_decimal(
        inputs
            .encoded()
            .get("public_hash")
            .expect("public hash input"),
    )
    .expect("public hash");
    spr_program::verifier::verify_groth16(
        &proof,
        spr_program::to_bytes(&public_hash),
        &verifying_key.as_key(),
    )
    .expect("valid swap proof");
    let keys_dir = keys_dir().join(id.name());
    let row = CircuitRow {
        name: id.name().into(),
        constraints: u64::try_from(constraint_count(id).expect("constraint count"))
            .expect("fits u64"),
        public_inputs: u64::try_from(verifying_key.as_key().nr_pubinputs).expect("fits u64"),
        proving_key_bytes: file_size(&keys_dir.join("pk.bin")),
        verifying_key_bytes: file_size(&keys_dir.join("vk.bin")),
        proof_bytes: proof_bytes(&proof.a, &proof.b, &proof.c),
        key_load,
        warm_proof,
    };
    (proof, row)
}

/// The fixture tree holding the transaction's real inputs in order, and their
/// spend proofs.
fn tree_fixture(tree: &Pubkey, proof_inputs: &SppProofInputs) -> (Account, Vec<SpendProof>) {
    let commitments = proof_inputs.input_utxo_hashes().expect("input commitments");
    let leaves: Vec<[u8; 32]> = commitments.iter().map(|input| input.utxo_hash).collect();
    let (tree_account, utxo_root, nullifier_root, root_index) = build_tree_fixture(tree, &leaves);
    let state_tree = local_state_tree(&leaves);
    assert_eq!(state_tree.root(), utxo_root, "state root gate");
    let nf_tree = nullifier_tree();
    assert_eq!(nf_tree.root(), nullifier_root, "nullifier root gate");
    let spend_proofs = build_spend_proofs(
        tree,
        &state_tree,
        &nf_tree,
        &commitments,
        utxo_root,
        nullifier_root,
        root_index,
    );
    (tree_account, spend_proofs)
}

fn file_size(path: &Path) -> u64 {
    fs::metadata(path)
        .unwrap_or_else(|e| panic!("stat {}: {e}", path.display()))
        .len()
}

// --- mollusk and SPP fixtures ----------------------------------------------

fn to_mollusk_pubkey(key: &Pubkey) -> Pubkey {
    Pubkey::new_from_array(key.to_bytes())
}

fn to_mollusk_instruction(ix: &Instruction) -> Instruction {
    Instruction {
        program_id: to_mollusk_pubkey(&ix.program_id),
        accounts: ix
            .accounts
            .iter()
            .map(|meta| AccountMeta {
                pubkey: to_mollusk_pubkey(&meta.pubkey),
                is_signer: meta.is_signer,
                is_writable: meta.is_writable,
            })
            .collect(),
        data: ix.data.clone(),
    }
}

fn mollusk_program_account(program_id: &Pubkey) -> (Pubkey, Account) {
    let account = mollusk_svm::program::create_program_account_loader_v3(program_id);
    (*program_id, account)
}

fn system_owned_account(lamports: u64) -> Account {
    Account {
        lamports,
        data: Vec::new(),
        owner: Pubkey::new_from_array([0u8; 32]),
        executable: false,
        rent_epoch: 0,
    }
}

fn build_tree_fixture(tree: &Pubkey, leaves: &[[u8; 32]]) -> (Account, [u8; 32], [u8; 32], u16) {
    let mut tree_account_bytes = vec![0u8; tree_account_size()];
    let root_index = leaves.len() as u16;
    let (utxo_root, nullifier_root) = {
        let mut account = TreeAccount::init(
            &mut tree_account_bytes,
            TREE_ACCOUNT_DISCRIMINATOR,
            STATE_HEIGHT as u8,
            tree.to_bytes(),
            BENCH_TREE_ID,
            nullifier_tree_params(),
            default_tree_fees(),
        )
        .expect("init tree account");
        for (slot, leaf) in (1_u64..).zip(leaves) {
            account
                .utxo_tree()
                .append(*leaf, slot)
                .expect("append leaf");
        }
        (
            account.get_utxo_tree_root(root_index).expect("utxo root"),
            account.get_nullifier_tree_root(0).expect("nullifier root"),
        )
    };
    let fixture = Account {
        lamports: 1_000_000_000_000,
        data: tree_account_bytes,
        owner: Pubkey::new_from_array(SHIELDED_POOL_PROGRAM_ID),
        executable: false,
        rent_epoch: 0,
    };
    (fixture, utxo_root, nullifier_root, root_index)
}

fn local_state_tree(leaves: &[[u8; 32]]) -> MerkleTree<Poseidon> {
    let mut tree = MerkleTree::<Poseidon>::new(STATE_TREE_HEIGHT, 0);
    for leaf in leaves {
        tree.append(leaf).expect("append state leaf");
    }
    tree
}

fn nullifier_tree() -> IndexedMerkleTree<Poseidon, usize> {
    let modulus_minus_one =
        BigUint::parse_bytes(BN254_MODULUS_DEC.as_bytes(), 10).expect("parse bn254 modulus") - 1u32;
    IndexedMerkleTree::<Poseidon, usize>::new_with_next_value(
        NULLIFIER_TREE_HEIGHT,
        0,
        modulus_minus_one,
    )
    .expect("nullifier tree")
}

fn build_spend_proofs(
    tree: &Pubkey,
    state_tree: &MerkleTree<Poseidon>,
    nf_tree: &IndexedMerkleTree<Poseidon, usize>,
    commitments: &[&spr_transaction::utxo::SppProofInputUtxo],
    utxo_root: [u8; 32],
    nullifier_root: [u8; 32],
    root_index: u16,
) -> Vec<SpendProof> {
    let merkle_context = MerkleContext {
        tree_type: 0,
        tree: Address::new_from_array(tree.to_bytes()),
    };
    commitments
        .iter()
        .enumerate()
        .map(|(leaf_index, commitment)| {
            let state_path = state_tree
                .get_proof_of_leaf(leaf_index, true)
                .expect("state proof")
                .to_vec();
            let nf = nf_tree
                .get_non_inclusion_proof(&BigUint::from_bytes_be(&commitment.nullifier))
                .expect("non inclusion proof");
            SpendProof {
                state: MerkleProof {
                    leaf: commitment.utxo_hash,
                    merkle_context: merkle_context.clone(),
                    path: state_path,
                    leaf_index: leaf_index as u64,
                    root: utxo_root,
                    root_seq: 0,
                    root_index,
                },
                nullifier: NonInclusionProof {
                    leaf: commitment.nullifier,
                    merkle_context: merkle_context.clone(),
                    path: nf.merkle_proof.to_vec(),
                    low_element: nf.leaf_lower_range_value,
                    low_element_index: nf.leaf_index as u64,
                    high_element: nf.leaf_higher_range_value,
                    high_element_index: 0,
                    root: nullifier_root,
                    root_seq: 0,
                    root_index: 0,
                },
            }
        })
        .collect()
}

fn assemble_accounts(
    ix: &Instruction,
    spp_id: &Pubkey,
    fixtures: &[(Pubkey, Account)],
) -> Vec<(Pubkey, Account)> {
    let spp = Pubkey::new_from_array(SHIELDED_POOL_PROGRAM_ID);
    ix.accounts
        .iter()
        .map(|meta| {
            if meta.pubkey == spp {
                mollusk_program_account(spp_id)
            } else if meta.pubkey == Pubkey::default() {
                mollusk_svm::program::keyed_account_for_system_program()
            } else if let Some((_, account)) = fixtures.iter().find(|(key, _)| *key == meta.pubkey)
            {
                (to_mollusk_pubkey(&meta.pubkey), account.clone())
            } else {
                (
                    to_mollusk_pubkey(&meta.pubkey),
                    system_owned_account(1_000_000_000),
                )
            }
        })
        .collect()
}

/// The shielded keypair of a Solana fee payer: one ed25519 identity.
fn keypair_from_payer(payer: &Keypair) -> ShieldedKeypair {
    let seed: [u8; 32] = payer.to_bytes()[..32]
        .try_into()
        .expect("ed25519 seed is the first 32 bytes");
    ShieldedKeypair::from_keypair(SigningKey::from_ed25519_bytes(&seed))
        .expect("keypair from payer")
}

/// Start the shared prover using the configured keys directory.
fn start_prover() {
    if env::var_os("ZOLANA_PROVER_KEYS_DIR").is_none() {
        let home = env::var("HOME").expect("HOME is set");
        env::set_var(
            "ZOLANA_PROVER_KEYS_DIR",
            Path::new(&home).join(".config/zolana/proving-keys"),
        );
    }
    spr_client::spawn_prover().expect("spawn prover");
}

/// The release shielded pool program: `SHIELDED_POOL_PROGRAM_PATH`, else
/// `shielded_pool_program.so` next to the profiled swap program.
fn shielded_pool_elf() -> Vec<u8> {
    let path = env::var("SHIELDED_POOL_PROGRAM_PATH").map_or_else(
        |_| Path::new(PROFILING_SBF_DIR).join("shielded_pool_program.so"),
        PathBuf::from,
    );
    fs::read(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}
