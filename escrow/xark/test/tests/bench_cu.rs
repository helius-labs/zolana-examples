//! CU benchmark of the timelock escrow escrow/withdraw instructions, replayed
//! under mollusk.
//!
//! The profiled program calls a profiler syscall that a validator does not
//! register, so it is built into its own directory, never `target/deploy`
//! (the e2e tests deploy from there). From `escrow/xark`:
//!
//! ```sh
//! cargo build-sbf --manifest-path program/Cargo.toml --tools-version v1.54 \
//!     --sbf-out-dir target/bench -- --features bpf-entrypoint,profile-program
//! SHIELDED_POOL_PROGRAM_PATH=/path/to/shielded_pool_program.so \
//! ZOLANA_CLI_BIN=/path/to/zolana \
//!     cargo test --release --locked -p timelock-escrow-test --test bench_cu -- --ignored --nocapture
//! ```
//!
//! - The shielded pool program is read from `SHIELDED_POOL_PROGRAM_PATH`,
//!   else from `target/bench/shielded_pool_program.so`.
//! - The SPP proofs come from the local prover server: one already serving
//!   `ZOLANA_PROVER_URL` is reused, else the `zolana` CLI (`ZOLANA_CLI_BIN`)
//!   starts one with the keys in `ZOLANA_PROVER_KEYS_DIR`, else
//!   `~/.config/zolana/proving-keys`.
//! - The escrow circuit proofs come from the program prover with the keys in
//!   `target/zk/timelock-escrow-program`.
//!
//! The SPP tree account is built directly: the program's tree init, then the
//! input UTXO hashes appended. The SPP proofs prove against local copies of
//! the state tree and the (empty) nullifier tree, whose roots the tree
//! account holds.
use std::{
    env, fs,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use light_program_profiler::{
    mollusk::{register_profiling_syscalls, take_profiling_entries},
    report::{CuBenchmark, ReadmeConfig, SectionTable},
};
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
use spr_program::ProgramProofInputs;
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
#[ignore = "CU benchmark; slow, needs the profiling SBF build and a prover. See the module docs"]
/// 1. Configure the program runtime.
/// 2. Start the prover and check the program keys.
/// 3. Measure both instructions and write the report.
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

    let report = CuBenchmark::new(ReadmeConfig {
        title: "Timelock Escrow (Xark) -- CU Benchmark".into(),
        description: "1. Measure the timelock escrow escrow/withdraw instructions under Mollusk using the shielded-pool program built from Zolana af9d32923 and the Xark-backed SDK. Each instruction verifies its generated program proof, then CPIs SPP `transact` (the `cpi_spp_transact_signed` row). Only the timelock escrow program is profiled; the shielded-pool program is built plain, so the CU its CPI consumes is charged to the `cpi_spp_transact_signed` row as a black box.\n\n2. Record SPP and escrow circuit proving times, consumed compute units, and serialized v1 transaction sizes. Escrow uses 1x2 (change and escrow UTXO); withdraw uses 1x2 with compact output padding.\n\n3. Report compiled circuit constraints, checksummed Xark proving-key and verifying-key sizes. Proof times are warm, in the release profile. An escrow circuit proof time is the median of five Xark proofs after one warm-up proof, from one checked circuit copy. An SPP proof time is one sample after one warm-up proof on the running prover server. Build the profiling program first with cargo build-sbf --manifest-path program/Cargo.toml --tools-version v1.54 --sbf-out-dir target/bench -- --features bpf-entrypoint,profile-program.".into(),
        output_path: OUTPUT_PATH.into(),
        regenerate_command: Some(
            "cargo test --release --locked -p timelock-escrow-test --test bench_cu -- --ignored --nocapture".into(),
        ),
        ..Default::default()
    });

    // 2. Start the prover and check the program keys.
    start_prover();
    for circuit in CircuitId::ALL {
        proof::verifying_key(circuit).expect("preload escrow keys");
    }

    let mut bench = Bench {
        mollusk,
        tree: spr_interface::pda::tree(BENCH_TREE_ID),
        prover: ProverClient::local(),
        report,
        circuit_proof_times: Vec::new(),
    };
    // 3. Measure both instructions and write the report.
    bench_escrow(&mut bench);
    bench_withdraw(&mut bench);
    let circuits = circuits_table(&bench.circuit_proof_times);
    bench.report.add_table("circuits", circuits);

    bench.report.generate().expect("write BENCHMARK.md");
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

/// Prove the PDA transaction against fixture trees after warming the prover.
fn prove_spp(bench: &Bench, proof_inputs: SppProofInputs) -> (Account, TransactIxData, Duration) {
    let private_tx_hash = proof_inputs
        .padding_independent_private_tx_hash()
        .expect("private transaction hash");
    let (tree_account, spend_proofs) = spend_fixture(&bench.tree, &proof_inputs);
    let dummies = dummy_proofs(&proof_inputs);
    let key = NullifierKey::from_secret([0; 31]);
    bench
        .prover
        .prove_transact(proof_inputs.clone(), &spend_proofs, &dummies, &key)
        .expect("warm prove transact");
    let start = Instant::now();
    let transact = bench
        .prover
        .prove_transact(proof_inputs, &spend_proofs, &dummies, &key)
        .expect("prove transact");
    let elapsed = start.elapsed();
    assert_eq!(transact.private_tx_hash, private_tx_hash);
    (tree_account, transact, elapsed)
}

/// Warm escrow circuit proofs sampled per instruction.
const WARM_PROOF_SAMPLES: usize = 5;

/// Median time of `WARM_PROOF_SAMPLES` Xark proofs after one warm-up proof,
/// all from one checked circuit copy.
fn warm_proof_time(circuit: CircuitId, inputs: &ProgramProofInputs) -> Duration {
    let (_private, loaded) = proof::checked_circuit(circuit).expect("checked circuit");
    loaded.prove(inputs.clone()).expect("warm-up circuit proof");
    let mut samples: Vec<Duration> = (0..WARM_PROOF_SAMPLES)
        .map(|_| {
            let start = Instant::now();
            loaded.prove(inputs.clone()).expect("circuit proof");
            start.elapsed()
        })
        .collect();
    samples.sort_unstable();
    samples
        .get(WARM_PROOF_SAMPLES / 2)
        .copied()
        .expect("warm proof samples")
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

fn proving_time_table(spp: Duration, circuit: Duration) -> SectionTable {
    SectionTable {
        title: "Proving Time".into(),
        headers: vec![
            "Warm SPP proof".into(),
            "Warm escrow circuit proof".into(),
            "Total".into(),
        ],
        rows: vec![vec![
            format!("{} ms", spp.as_millis()),
            format!("{} ms", circuit.as_millis()),
            format!("{} ms", (spp + circuit).as_millis()),
        ]],
    }
}

fn file_size(path: &Path) -> u64 {
    fs::metadata(path)
        .unwrap_or_else(|e| panic!("stat {}: {e}", path.display()))
        .len()
}

/// Bytes in decimal units: `716 B`, `1.33 MB`.
fn format_size(bytes: u64) -> String {
    if bytes < 1_000_000 {
        format!("{bytes} B")
    } else {
        format!("{:.2} MB", bytes as f64 / 1_000_000.0)
    }
}

/// Each circuit's constraint count, key sizes and warm proof time.
fn circuits_table(proof_times: &[(CircuitId, Duration)]) -> SectionTable {
    let rows = CircuitId::ALL
        .iter()
        .map(|&circuit| {
            let name = circuit.name();
            let count = proof::constraint_count(circuit)
                .expect("constraint count")
                .to_string();
            let keys_dir = proof::keys_dir();
            let proof_time = proof_times
                .iter()
                .find(|(proven, _)| *proven == circuit)
                .map(|(_, elapsed)| elapsed)
                .unwrap_or_else(|| panic!("no proof time for {name}"));
            vec![
                name.into(),
                count,
                format_size(file_size(&keys_dir.join(name).join("pk.bin"))),
                format_size(file_size(&keys_dir.join(name).join("vk.bin"))),
                format!("{} ms", proof_time.as_millis()),
            ]
        })
        .collect();
    SectionTable {
        title: String::new(),
        headers: vec![
            "Circuit".into(),
            "Constraints".into(),
            "Proving key".into(),
            "Verifying key".into(),
            "Warm proof time".into(),
        ],
        rows,
    }
}

/// The version 1 transaction this instruction is sent as. Its compute
/// ceilings live in the message header. Panics if it exceeds 4096 bytes.
fn tx_size_table(ix: &Instruction, payer: &Address) -> SectionTable {
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

    SectionTable {
        title: "Transaction Size".into(),
        headers: vec!["Instruction Data".into(), "Accounts".into(), "v1 Tx".into()],
        rows: vec![vec![
            format!("{} bytes", ix.data.len()),
            ix.accounts.len().to_string(),
            format!("{} bytes", v1),
        ]],
    }
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
    report: CuBenchmark,
    /// The warm circuit proof time of each circuit.
    circuit_proof_times: Vec<(CircuitId, Duration)>,
}

impl Bench {
    /// Check the instruction fits its budget and record its measurements.
    fn record(
        &mut self,
        name: &str,
        circuit: CircuitId,
        ix: &Instruction,
        payer: &Address,
        tree_account: Account,
        (spp, circuit_dur): (Duration, Duration),
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
        self.report.add_from_entries(name, entries);
        self.report
            .add_table(name, proving_time_table(spp, circuit_dur));
        self.report.add_table(name, tx_size_table(ix, payer));
        self.circuit_proof_times.push((circuit, circuit_dur));
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
    let circuit_dur = warm_proof_time(CircuitId::Escrow, &transaction.proof_inputs);
    let escrow_proof =
        proof::prove(CircuitId::Escrow, &transaction.proof_inputs).expect("escrow prove");

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
        CircuitId::Escrow,
        &ix,
        &payer.pubkey(),
        tree_account,
        (spp_dur, circuit_dur),
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
    let circuit_dur = warm_proof_time(CircuitId::Withdraw, &transaction.proof_inputs);
    let withdraw_proof =
        proof::prove(CircuitId::Withdraw, &transaction.proof_inputs).expect("withdraw prove");

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
        CircuitId::Withdraw,
        &ix,
        &payer.pubkey(),
        tree_account,
        (spp_dur, circuit_dur),
    );
}
