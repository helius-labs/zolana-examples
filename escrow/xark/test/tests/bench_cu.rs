//! Benchmark of the timelock escrow escrow/withdraw instructions in the shared
//! benchmark format (`bench_report`): CU replayed under mollusk, warm proving
//! times, key loads, circuit sizes and transaction sizes. It writes
//! `BENCHMARK.md`; never edit that file by hand.
//!
//! The profiled program calls a profiler syscall that a validator does not
//! register, so it is built into its own directory, never `target/deploy`
//! (the e2e tests deploy from there). From `escrow/xark`:
//!
//! ```sh
//! cargo build-sbf --manifest-path program/Cargo.toml --tools-version v1.54 \
//!     --sbf-out-dir target/bench -- --locked --features bpf-entrypoint,profile-program
//! SHIELDED_POOL_PROGRAM_PATH=/path/to/shielded_pool_program.so \
//! ZOLANA_CLI_BIN=/path/to/zolana \
//!     cargo test --release --manifest-path test/Cargo.toml --test bench_cu -- --ignored --nocapture
//! ```
//!
//! - The shielded pool program is read from `SHIELDED_POOL_PROGRAM_PATH`,
//!   else from `target/bench/shielded_pool_program.so`.
//! - The SPP proofs come from the local prover server: one already serving
//!   `ZOLANA_PROVER_URL` is reused, else the `zolana` CLI (`ZOLANA_CLI_BIN`)
//!   starts one with the keys in `ZOLANA_PROVER_KEYS_DIR`, else
//!   `~/.config/zolana/proving-keys`.
//! - The escrow circuit proofs come from the program prover with the keys in
//!   `target/zk/timelock-escrow-program`. Each cold key load runs in a child
//!   process of the bench (`key_load_probe`).
//!
//! The SPP tree account is built directly: the program's tree init, then the
//! input UTXO hashes appended. The SPP proofs prove against local copies of
//! the state tree and the (empty) nullifier tree, whose roots the tree
//! account holds.
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
use solana_instruction::Instruction;
use solana_keypair::Keypair;
use solana_signer::Signer;
use spr_client::{
    transaction_size, ComputeBudgetConfig, MerkleContext, MerkleProof, NonInclusionProof,
    ProverClient, ProverExt, SpendProof, STATE_TREE_HEIGHT,
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
use spr_keypair::{random_blinding, NullifierKey, ShieldedKeypair};
use spr_merkle_tree::{indexed::IndexedMerkleTree, MerkleTree};
use spr_program::{proof::compressed_proof, CompressedProof, ProgramProofInputs};
use spr_test_utils::{transact::nullifier_tree, utxo::dummy_proofs};
use spr_transaction::{instructions::transact::SppProofInputs, Mint, SppProofOutputUtxo};
use spr_tree::TreeAccount;
use timelock_escrow_sdk::{
    escrow_authority,
    instructions::{
        escrow::{Escrow, EscrowTransactionParams},
        withdraw::{Withdraw, WithdrawTransactionParams},
    },
    proof::{self, CircuitId},
    state::{EscrowTerms, EscrowUtxo},
};

const PROFILING_SBF_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../target/bench");
const OUTPUT_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../BENCHMARK.md");
/// The whole per-transaction budget, which every instruction must fit.
const TRANSACT_CU_LIMIT: u32 = 1_400_000;
/// The version 1 transaction size limit.
const V1_TX_LIMIT: usize = 4096;

/// Raw id of the fixture tree. `build_tree_fixture` initializes the account
/// with this id, and every UTXO commitment folds it in.
const BENCH_TREE_ID: u16 = 0;
/// The mollusk slot, past every fixture leaf's append slot.
const BENCH_SLOT: u64 = 1_000;
const INPUT_AMOUNT: u64 = 1_000_000;
const LOCK_AMOUNT: u64 = 400_000;
const UNLOCK_TIMESTAMP: u64 = 1_000_000;

#[test]
#[ignore = "benchmark; slow, needs the profiling SBF build and a prover. See the module docs"]
/// 1. Configure the program runtime.
/// 2. Start the prover and check the program keys.
/// 3. Measure both instructions and both circuits, and write the report.
fn bench_cu_escrow() {
    // 1. Configure the program runtime.
    env::set_var("SBF_OUT_DIR", PROFILING_SBF_DIR);

    let escrow_id = timelock_escrow_program::ID;
    let spp_id = Address::new_from_array(SHIELDED_POOL_PROGRAM_ID);

    let mut mollusk = Mollusk::default();
    mollusk.compute_budget.compute_unit_limit = u64::from(TRANSACT_CU_LIMIT);
    // Keep the runtime slot after every fixture append.
    mollusk.warp_to_slot(BENCH_SLOT);
    mollusk.sysvars.clock.unix_timestamp = i64::try_from(UNLOCK_TIMESTAMP).expect("fits i64") + 1;
    register_profiling_syscalls(&mut mollusk);
    mollusk.add_program(&escrow_id, "timelock_escrow_program");
    mollusk.add_program_with_loader_and_elf(&spp_id, &LOADER_V3, &shielded_pool_elf());

    // 2. Start the prover and check the program keys.
    start_prover();
    for circuit in CircuitId::ALL {
        proof::verifying_key(circuit).expect("preload escrow keys");
    }

    let mut bench = Bench {
        mollusk,
        tree: spr_interface::pda::tree(BENCH_TREE_ID),
        prover: ProverClient::local(),
        circuits: Vec::new(),
        instructions: Vec::new(),
    };
    // 3. Measure both instructions and both circuits, and write the report.
    bench_escrow(&mut bench);
    bench_withdraw(&mut bench);

    Report {
        example: "Timelock Escrow",
        implementation: Implementation::Xark,
        circuits: bench.circuits,
        instructions: bench.instructions,
    }
    .write(OUTPUT_PATH);
}

fn system_owned_account(lamports: u64) -> Account {
    Account::new(lamports, 0, &Address::default())
}

fn merkle_context(tree: &Address) -> MerkleContext {
    MerkleContext {
        tree_type: 0,
        tree: *tree,
    }
}

fn build_tree_fixture(tree: &Address, leaves: &[[u8; 32]]) -> (Account, [u8; 32], [u8; 32], u16) {
    let mut tree_account_bytes = vec![0u8; tree_account_size()];
    let root_index = u16::try_from(leaves.len()).expect("leaf count fits a root index");
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
        owner: Address::new_from_array(SHIELDED_POOL_PROGRAM_ID),
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

fn non_inclusion_proof(
    tree: &Address,
    nf_tree: &IndexedMerkleTree<Poseidon, usize>,
    leaf: [u8; 32],
) -> NonInclusionProof {
    let nf = nf_tree
        .get_non_inclusion_proof(&BigUint::from_bytes_be(&leaf))
        .expect("non inclusion proof");
    NonInclusionProof {
        leaf,
        merkle_context: merkle_context(tree),
        path: nf.merkle_proof.to_vec(),
        low_element: nf.leaf_lower_range_value,
        low_element_index: nf.leaf_index as u64,
        high_element: nf.leaf_higher_range_value,
        high_element_index: 0,
        root: nf_tree.root(),
        root_seq: 0,
        root_index: 0,
    }
}

/// The tree account holding the inputs of `proof_inputs` and their spend
/// proofs. The real inputs sit at leaves `0..n` in input order, which the
/// callers pick as their leaf indices.
fn spend_fixture(tree: &Address, proof_inputs: &SppProofInputs) -> (Account, Vec<SpendProof>) {
    let commitments = proof_inputs.input_utxo_hashes().expect("input commitments");
    let leaves: Vec<[u8; 32]> = commitments.iter().map(|input| input.utxo_hash).collect();
    let (tree_account, utxo_root, nullifier_root, root_index) = build_tree_fixture(tree, &leaves);
    let state_tree = local_state_tree(&leaves);
    assert_eq!(state_tree.root(), utxo_root, "state root gate");
    let nf_tree = nullifier_tree().expect("nullifier tree");
    assert_eq!(nf_tree.root(), nullifier_root, "nullifier root gate");
    let merkle_context = merkle_context(tree);
    let spend_proofs = commitments
        .iter()
        .zip(0_u64..)
        .map(|(commitment, leaf_index)| {
            assert_eq!(commitment.leaf_index, leaf_index, "inputs at leaves 0..n");
            let state_path = state_tree
                .get_proof_of_leaf(leaf_index as usize, true)
                .expect("state proof")
                .to_vec();
            SpendProof {
                state: MerkleProof {
                    leaf: commitment.utxo_hash,
                    merkle_context: merkle_context.clone(),
                    path: state_path,
                    leaf_index,
                    root: utxo_root,
                    root_seq: 0,
                    root_index,
                },
                nullifier: non_inclusion_proof(tree, &nf_tree, commitment.nullifier),
            }
        })
        .collect();
    (tree_account, spend_proofs)
}

fn assemble_accounts(ix: &Instruction, fixtures: &[(Address, Account)]) -> Vec<(Address, Account)> {
    let spp = Address::new_from_array(SHIELDED_POOL_PROGRAM_ID);
    ix.accounts
        .iter()
        .map(|meta| {
            if meta.pubkey == spp {
                (
                    spp,
                    mollusk_svm::program::create_program_account_loader_v3(&spp),
                )
            } else if meta.pubkey == Address::default() {
                mollusk_svm::program::keyed_account_for_system_program()
            } else if let Some((_, account)) = fixtures.iter().find(|(key, _)| *key == meta.pubkey)
            {
                (meta.pubkey, account.clone())
            } else {
                (meta.pubkey, system_owned_account(1_000_000_000))
            }
        })
        .collect()
}
/// Prove the PDA transaction against fixture trees: one warm-up proof, then
/// the median time of the timed proofs on the running prover server.
fn prove_spp(bench: &Bench, proof_inputs: SppProofInputs) -> (Account, TransactIxData, Duration) {
    let private_tx_hash = proof_inputs
        .padding_independent_private_tx_hash()
        .expect("private transaction hash");
    let (tree_account, spend_proofs) = spend_fixture(&bench.tree, &proof_inputs);
    let dummies = dummy_proofs(&proof_inputs);
    let key = NullifierKey::from_secret([0; 31]);
    let prove = || {
        bench
            .prover
            .prove_transact(proof_inputs.clone(), &spend_proofs, &dummies, &key)
            .expect("prove transact")
    };
    let (transact, warm) = warm_median(prove);
    assert_eq!(transact.private_tx_hash, private_tx_hash);
    (tree_account, transact, warm)
}

/// Measure `circuit` on `inputs`: its cold key loads, its warm proof time
/// from one loaded checked circuit copy, and its sizes. The last proof,
/// checked against the verifying key, is returned for the instruction.
fn measure_circuit(
    circuit: CircuitId,
    inputs: &ProgramProofInputs,
) -> (CompressedProof, CircuitRow) {
    let (key_load, _) = cold_key_load(circuit.name());
    let (_private, loaded) = proof::checked_circuit(circuit).expect("checked circuit");
    let (proven, warm_proof) = warm_median(|| loaded.prove(inputs.clone()).expect("circuit proof"));
    let proof = compressed_proof(&proven.proof).expect("compressed proof");
    let verifying_key = proof::verifying_key(circuit).expect("verifying key");
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
    .expect("valid circuit proof");
    let keys_dir = proof::keys_dir().join(circuit.name());
    let row = CircuitRow {
        name: circuit.name().into(),
        constraints: u64::try_from(proof::constraint_count(circuit).expect("constraint count"))
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

/// The child-process half of the cold key load: one load of the checked
/// bytecode and proving key of the circuit `cold_key_load` names.
#[test]
#[ignore = "child process of bench_cu_escrow"]
fn key_load_probe() {
    time_key_load(|name| {
        let circuit = CircuitId::ALL
            .into_iter()
            .find(|circuit| circuit.name() == name)
            .expect("known circuit");
        proof::checked_circuit(circuit).expect("load checked circuit")
    });
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

/// The shielded pool program: `SHIELDED_POOL_PROGRAM_PATH`, else
/// `shielded_pool_program.so` next to the profiled escrow program.
fn shielded_pool_elf() -> Vec<u8> {
    let path = env::var("SHIELDED_POOL_PROGRAM_PATH").map_or_else(
        |_| Path::new(PROFILING_SBF_DIR).join("shielded_pool_program.so"),
        PathBuf::from,
    );
    std::fs::read(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

fn file_size(path: &Path) -> u64 {
    fs::metadata(path)
        .unwrap_or_else(|e| panic!("stat {}: {e}", path.display()))
        .len()
}

/// The serialized size of the version 1 transaction this instruction is sent
/// as. Its compute ceilings live in the message header. Panics if it exceeds
/// 4096 bytes.
fn v1_tx_bytes(ix: &Instruction, payer: &Address) -> u64 {
    let v1 = transaction_size(
        payer,
        std::slice::from_ref(ix),
        ComputeBudgetConfig::new(TRANSACT_CU_LIMIT),
    )
    .expect("measure v1 transaction")
    .bytes;
    assert!(
        v1 <= V1_TX_LIMIT,
        "v1 transaction of {v1} bytes exceeds {V1_TX_LIMIT}"
    );
    u64::try_from(v1).expect("fits u64")
}

/// A creator whose Solana key pays the transactions.
fn creator() -> (Keypair, ShieldedKeypair) {
    let payer = Keypair::new();
    let shielded = ShieldedKeypair::from_keypair(&payer).expect("shielded keypair");
    (payer, shielded)
}

fn terms(creator: &ShieldedKeypair) -> EscrowTerms {
    EscrowTerms {
        creator: creator.shielded_address().expect("creator address"),
        unlock_timestamp: UNLOCK_TIMESTAMP,
    }
}
struct Bench {
    mollusk: Mollusk,
    tree: Address,
    prover: ProverClient,
    circuits: Vec<CircuitRow>,
    instructions: Vec<InstructionRow>,
}

impl Bench {
    /// Check the instruction succeeds within its budget and record its
    /// measurements and its circuit's.
    fn record(
        &mut self,
        name: &str,
        circuit: CircuitRow,
        ix: &Instruction,
        payer: &Address,
        tree_account: Account,
        warm_spp_proof: Duration,
    ) {
        let fixtures = vec![
            (self.tree, tree_account),
            (*payer, system_owned_account(100_000_000_000)),
        ];
        let accounts = assemble_accounts(ix, &fixtures);
        self.mollusk
            .process_and_validate_instruction(ix, &accounts, &[Check::success()]);
        let entries = take_profiling_entries();
        assert!(!entries.is_empty(), "no profiling entries for '{name}'");
        self.instructions.push(InstructionRow {
            name: name.into(),
            circuit: circuit.name.clone(),
            entries,
            warm_spp_proof,
            instruction_data_bytes: u64::try_from(ix.data.len()).expect("fits u64"),
            accounts: u64::try_from(ix.accounts.len()).expect("fits u64"),
            v1_tx_bytes: v1_tx_bytes(ix, payer),
        });
        self.circuits.push(circuit);
    }
}

fn bench_escrow(bench: &mut Bench) {
    let (payer, creator) = creator();
    let authority = escrow_authority();
    let source = authority
        .input(
            &SppProofOutputUtxo {
                asset: Mint::SOL,
                amount: INPUT_AMOUNT,
                blinding: random_blinding(),
                owner_address: Some(
                    authority
                        .address(creator.viewing_pubkey())
                        .expect("authority address"),
                ),
                ..Default::default()
            },
            BENCH_TREE_ID,
            0,
        )
        .expect("source input");
    let transaction = EscrowTransactionParams {
        terms: terms(&creator),
        amount: LOCK_AMOUNT,
        source,
        payer: payer.pubkey(),
        tree_id: BENCH_TREE_ID,
    }
    .build(&creator)
    .expect("build escrow");
    let (tree_account, spp_proof, spp_dur) = prove_spp(bench, transaction.spp_proof_inputs.clone());
    let (escrow_proof, circuit) = measure_circuit(CircuitId::Escrow, &transaction.proof_inputs);

    let ix = Escrow {
        payer: payer.pubkey(),
        tree: bench.tree,
        escrow_proof,
        spp_proof,
    }
    .instruction()
    .expect("escrow instruction");

    bench.record(
        "escrow",
        circuit,
        &ix,
        &payer.pubkey(),
        tree_account,
        spp_dur,
    );
}

fn bench_withdraw(bench: &mut Bench) {
    let (payer, creator) = creator();
    let escrow_utxo = EscrowUtxo {
        terms: terms(&creator),
        blinding: random_blinding(),
        asset: Mint::SOL,
        amount: LOCK_AMOUNT,
    };
    let transaction = WithdrawTransactionParams {
        escrow_utxo,
        tree_id: BENCH_TREE_ID,
        leaf_index: 0,
        payer: payer.pubkey(),
    }
    .build(&creator)
    .expect("build withdraw");
    let (tree_account, spp_proof, spp_dur) = prove_spp(bench, transaction.spp_proof_inputs.clone());
    let (withdraw_proof, circuit) = measure_circuit(CircuitId::Withdraw, &transaction.proof_inputs);

    let ix = Withdraw {
        creator: payer.pubkey(),
        payer: payer.pubkey(),
        tree: bench.tree,
        withdraw_proof,
        unlock_timestamp: UNLOCK_TIMESTAMP,
        spp_proof,
    }
    .instruction()
    .expect("withdraw instruction");

    bench.record(
        "withdraw",
        circuit,
        &ix,
        &payer.pubkey(),
        tree_account,
        spp_dur,
    );
}
