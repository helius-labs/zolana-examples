//! Benchmark of the timelock escrow escrow/withdraw instructions in the shared
//! benchmark format (`bench_report`): CU replayed under mollusk, warm proving
//! times, key loads, circuit sizes and transaction sizes. It writes
//! `BENCHMARK.md`; never edit that file by hand.
//!
//! The profiled program calls a profiler syscall that a validator does not
//! register, so it is built into its own directory, never `target/deploy`.
//! From `escrow/gnark`, after generating the circuit keys into `build/gnark`
//! (see the README):
//!
//! ```sh
//! CARGO_TARGET_DIR="$PWD/target" cargo build-sbf --manifest-path program/Cargo.toml --tools-version v1.54 \
//!     --sbf-out-dir target/bench -- --features bpf-entrypoint,profile-program
//! # ZOLANA_CLI_BIN, SHIELDED_POOL_PROGRAM_PATH, PROVER_BIN, ...
//! export $(zolana dev artifacts)
//! cargo test --release --manifest-path test/Cargo.toml --test bench_cu -- --ignored --nocapture
//! ```
//!
//! `zolana` is the `cli-v0.10.0` release CLI (see
//! `test-utils/run-release-tests.sh`); `dev artifacts` downloads the deployed
//! shielded-pool v1.0.0 program and the prover it pins.
//!
//! - The shielded pool program is read from `SHIELDED_POOL_PROGRAM_PATH`,
//!   else from `target/bench/shielded_pool_program.so`.
//! - The SPP proofs come from the local prover server: one already serving
//!   `ZOLANA_PROVER_URL` is reused, else the `zolana` CLI (`ZOLANA_CLI_BIN`)
//!   starts one with the keys in `ZOLANA_PROVER_KEYS_DIR`, else
//!   `~/.config/zolana/proving-keys`.
//! - The escrow circuit proofs come from the in-process gnark prover with the
//!   keys in `build/gnark`. Each cold key load runs in a child process of the
//!   bench (`key_load_probe`).
//!
//! The SPP tree account is built directly: the program's tree init, then the
//! input UTXO hashes appended. The SPP proofs prove against local copies of
//! the state tree and the (empty) nullifier tree, whose roots the tree
//! account holds.
#[path = "../../../../test-utils/bench_report.rs"]
mod bench_report;
#[path = "../../../../test-utils/gnark_circuit_size.rs"]
mod gnark_circuit_size;

use std::{
    env, fs,
    path::{Path, PathBuf},
    time::Duration,
};

use bench_report::{
    cold_key_load, proof_bytes, time_key_load, warm_median, CircuitRow, Implementation,
    InstructionRow, Report, INPUT_BLINDING,
};
use gnark_circuit_size::circuit_size;
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
use spr_gnark_ffi_prover::Circuit;
use spr_hasher::Poseidon;
use spr_interface::{
    instruction::instruction_data::transact::TransactIxData,
    state::{
        default_tree_fees, discriminator::TREE_ACCOUNT_DISCRIMINATOR, nullifier_tree_params,
        tree_account_size, STATE_HEIGHT,
    },
    SHIELDED_POOL_PROGRAM_ID,
};
use spr_keypair::{random_blinding, ShieldedKeypair, SigningKey};
use spr_merkle_tree::{indexed::IndexedMerkleTree, MerkleTree};
use spr_test_utils::utxo::{
    encrypt_transaction_data, get_transaction_viewing_key, prepare_output_blindings,
};
use spr_transaction::utxo::SppProofInputUtxo;
use spr_transaction::{
    instructions::transact::{ExternalData, SppProofInputs, SppProofOutputUtxo, BN254_MODULUS_DEC},
    Data, Utxo,
};
use spr_tree::TreeAccount;
use timelock_escrow_prover::{CircuitId, TimelockProof, PROVER};
use timelock_escrow_sdk::{
    instructions::{
        escrow::{Escrow, EscrowProofInputParams, SppTxHashes},
        withdraw::{Withdraw, WithdrawProofInputParams},
    },
    prover::EscrowProverClient,
    shared::input_sum,
    state::{EscrowTerms, EscrowUtxo},
};

const PROFILING_SBF_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../target/bench");
const OUTPUT_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../BENCHMARK.md");
/// The whole per-transaction budget, which every instruction must fit.
const TRANSACT_CU_LIMIT: u32 = 1_400_000;
/// The version 1 transaction size limit.
const V1_TX_LIMIT: usize = 4096;
/// The mollusk slot, past every fixture leaf's append slot.
const BENCH_SLOT: u64 = 1_000;
/// Keypair seeds of the escrow and withdraw creators. Fixed keys make every
/// run replay the same transactions.
const ESCROW_CREATOR_SEED: u8 = 1;
const WITHDRAW_CREATOR_SEED: u8 = 2;
/// Blinding of the dummy input, whose nullifier the SPP records too.
const DUMMY_BLINDING: [u8; 32] = bench_report::fixed_blinding(6);

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

/// Raw id of the fixture tree. `build_tree_fixture` initializes the account with
/// this id, and every UTXO commitment folds it in.
const BENCH_TREE_ID: u16 = 0;

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
    commitments: &[&SppProofInputUtxo],
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

fn keypair_from_payer(payer: &Keypair) -> ShieldedKeypair {
    let seed: [u8; 32] = payer.to_bytes()[..32]
        .try_into()
        .expect("ed25519 seed is the first 32 bytes");
    ShieldedKeypair::from_keypair(SigningKey::from_ed25519_bytes(&seed))
        .expect("keypair from payer")
}

/// Prove the transaction against fixture trees: one warm-up proof, then the
/// median time of the timed proofs on the running prover server.
fn prove_transact_warm(
    proof_inputs: SppProofInputs,
    spend_proofs: &[SpendProof],
    prover: &ProverClient,
    keys: &[&spr_keypair::NullifierKey],
) -> (TransactIxData, Duration) {
    let dummy_proofs = spr_test_utils::utxo::dummy_proofs(&proof_inputs);
    let prove = || {
        prover
            .prove_transact(
                proof_inputs.clone(),
                spend_proofs,
                &dummy_proofs,
                &spr_test_utils::utxo::ProofKeys(keys),
            )
            .expect("prove transact")
    };
    warm_median(prove)
}

/// Measure `circuit`: its cold key loads, its warm proof time with the keys
/// preloaded into the in-process prover, and its sizes. `prove` proves the
/// instruction's inputs once; the last proof is returned for the instruction.
fn measure_circuit(
    circuit: CircuitId,
    prove: impl FnMut() -> TimelockProof,
) -> (TimelockProof, CircuitRow) {
    let (key_load, key_load_outputs) = cold_key_load(circuit.name());
    let keys_dir = PROVER.keys_dir(circuit);
    let vk = fs::read(keys_dir.join("vk.bin")).expect("read verifying key");
    let size = circuit_size(circuit.name(), &key_load_outputs, &vk);
    PROVER.preload(circuit).expect("preload circuit keys");
    let (proof, warm_proof) = warm_median(prove);
    let row = CircuitRow {
        name: circuit.name().into(),
        constraints: size.constraints,
        public_inputs: size.public_inputs,
        proving_key_bytes: file_size(&keys_dir.join("pk.bin")),
        verifying_key_bytes: u64::try_from(vk.len()).expect("fits u64"),
        proof_bytes: proof_bytes(&proof.proof_a, &proof.proof_b, &proof.proof_c),
        key_load,
        per_proof_setup: Duration::ZERO,
        warm_proof,
        in_memory_proof: warm_proof,
    };
    (proof, row)
}

/// The child-process half of the cold key load: one preload of the circuit
/// `cold_key_load` names (compile the constraint system, read the proving key).
#[test]
#[ignore = "child process of bench_cu_escrow"]
fn key_load_probe() {
    time_key_load(|name| {
        let circuit = *CircuitId::ALL
            .iter()
            .find(|circuit| circuit.name() == name)
            .expect("known circuit");
        PROVER.preload(circuit).expect("preload circuit keys");
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
    fs::read(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

fn file_size(path: &Path) -> u64 {
    fs::metadata(path)
        .unwrap_or_else(|e| panic!("stat {}: {e}", path.display()))
        .len()
}

/// The serialized size of the version 1 transaction this instruction is sent
/// as. Its compute ceilings live in the message header. Panics if it exceeds
/// 4096 bytes.
fn v1_tx_bytes(ix: &Instruction, payer: &Pubkey) -> u64 {
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

struct Bench {
    mollusk: Mollusk,
    spp_id: Pubkey,
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
        fixtures: &[(Pubkey, Account)],
        warm_spp_proof: Duration,
    ) {
        let accounts = assemble_accounts(ix, &self.spp_id, fixtures);
        let mollusk_ix = to_mollusk_instruction(ix);
        self.mollusk
            .process_and_validate_instruction(&mollusk_ix, &accounts, &[Check::success()]);
        let entries = take_profiling_entries();
        assert!(!entries.is_empty(), "no profiling entries for '{name}'");
        let payer = ix
            .accounts
            .iter()
            .find(|meta| meta.is_signer)
            .expect("a signing payer")
            .pubkey;
        self.instructions.push(InstructionRow {
            name: name.into(),
            circuit: circuit.name.clone(),
            entries,
            warm_spp_proof,
            instruction_data_bytes: u64::try_from(ix.data.len()).expect("fits u64"),
            accounts: u64::try_from(ix.accounts.len()).expect("fits u64"),
            v1_tx_bytes: v1_tx_bytes(ix, &payer),
        });
        self.circuits.push(circuit);
    }
}

#[test]
#[ignore = "benchmark; slow, needs the profiling SBF build, the circuit keys and a prover. See the module docs"]
/// 1. Configure the program runtime.
/// 2. Start the prover.
/// 3. Measure both instructions and both circuits, and write the report.
fn bench_cu_escrow() {
    // 1. Configure the program runtime.
    env::set_var("SBF_OUT_DIR", PROFILING_SBF_DIR);

    let escrow_id = Pubkey::new_from_array(*timelock_escrow_program::ID.as_array());
    let spp_id = Pubkey::new_from_array(SHIELDED_POOL_PROGRAM_ID);

    let mut mollusk = Mollusk::default();
    mollusk.compute_budget.compute_unit_limit = u64::from(TRANSACT_CU_LIMIT);
    // Keep the runtime slot after every fixture append.
    mollusk.warp_to_slot(BENCH_SLOT);
    register_profiling_syscalls(&mut mollusk);
    mollusk.add_program(&escrow_id, "timelock_escrow_program");
    mollusk.add_program_with_loader_and_elf(&spp_id, &LOADER_V3, &shielded_pool_elf());

    // 2. Start the prover.
    start_prover();

    let mut bench = Bench {
        mollusk,
        spp_id,
        circuits: Vec::new(),
        instructions: Vec::new(),
    };
    // 3. Measure both instructions and both circuits, and write the report.
    bench_escrow(&mut bench);
    bench_withdraw(&mut bench);

    Report {
        example: "Timelock Escrow",
        implementation: Implementation::Gnark,
        circuits: bench.circuits,
        instructions: bench.instructions,
    }
    .write(OUTPUT_PATH);
}

fn bench_escrow(bench: &mut Bench) {
    const INPUT_AMOUNT: u64 = 1_000_000;
    const LOCK_AMOUNT: u64 = 400_000;
    const UNLOCK_TIMESTAMP: u64 = 1_000_000;

    let tree = spr_interface::pda::tree(BENCH_TREE_ID);
    let payer = Keypair::new_from_array([ESCROW_CREATOR_SEED; 32]);
    let creator = keypair_from_payer(&payer);
    let creator_address = creator.shielded_address().expect("creator address");

    // The source UTXO is the creator's own balance, which the creator signs
    // for as owner signer, as in the localnet test.
    let input_utxo = Utxo {
        owner: creator_address.signing_pubkey,
        asset: spr_transaction::Mint::SOL,
        amount: INPUT_AMOUNT,
        blinding: INPUT_BLINDING,
        ring_program_id: None,
        data: Data::default(),
    };
    let input_utxo: SppProofInputUtxo = spr_test_utils::utxo::wallet(
        input_utxo,
        &creator.nullifier_key,
        BENCH_TREE_ID,
        0,
        None,
        None,
    )
    .expect("indexed input")
    .into();
    let input_utxos = vec![
        input_utxo,
        SppProofInputUtxo::dummy_with_blinding(DUMMY_BLINDING, BENCH_TREE_ID).expect("dummy"),
    ];

    let mut escrow_utxo = EscrowUtxo {
        terms: EscrowTerms {
            creator: creator_address,
            unlock_timestamp: UNLOCK_TIMESTAMP,
        },
        blinding: random_blinding(),
        asset: spr_transaction::Mint::SOL,
        amount: LOCK_AMOUNT,
    };
    let escrow_output_utxo = escrow_utxo.output_utxo().expect("escrow output");

    let payer_address = Address::new_from_array(payer.pubkey().to_bytes());

    let escrow_asset = escrow_output_utxo.asset;
    let leftover =
        input_sum(&input_utxos, &escrow_asset.asset) - i128::from(escrow_output_utxo.amount);
    let change_amount =
        u64::try_from(leftover).expect("insufficient shielded balance for escrow bench");
    let change = SppProofOutputUtxo::new(escrow_asset, change_amount, creator_address)
        .expect("change output");
    let mut transaction_outputs = vec![change, escrow_output_utxo];
    let blinding_seed = prepare_output_blindings(&input_utxos, &mut transaction_outputs)
        .expect("derive escrow output blindings");
    let [change, escrow_output_utxo]: [_; 2] = transaction_outputs
        .try_into()
        .expect("escrow transaction has two outputs");
    // The escrow proof commits the escrow UTXO with its derived blinding.
    escrow_utxo.blinding = escrow_output_utxo.blinding;

    let transaction_viewing_key = get_transaction_viewing_key(&creator, &input_utxos)
        .expect("escrow transaction viewing key");
    let encoded = encrypt_transaction_data(
        &[change.clone(), escrow_output_utxo],
        &transaction_viewing_key,
        BENCH_TREE_ID,
    )
    .expect("encode escrow slots");

    let external_data = ExternalData::new(
        *transaction_viewing_key.pubkey().as_bytes(),
        encoded.salt,
        encoded.outputs,
        encoded.resolved_owner_tags,
        vec![],
    );
    let spp_proof_inputs = SppProofInputs {
        input_utxos,
        output_utxos: encoded.output_utxos,
        external_data,
        payer: payer_address,
        blinding_seed,
        output_tree_id: BENCH_TREE_ID,
        cache_accounts: Default::default(),
    };

    let commitments = spp_proof_inputs
        .input_utxo_hashes()
        .expect("input commitments");
    let leaves: Vec<[u8; 32]> = commitments.iter().map(|input| input.utxo_hash).collect();
    let (tree_account, utxo_root, nullifier_root, root_index) = build_tree_fixture(&tree, &leaves);
    let state_tree = local_state_tree(&leaves);
    assert_eq!(state_tree.root(), utxo_root, "state root gate");
    let nf_tree = nullifier_tree();
    assert_eq!(nf_tree.root(), nullifier_root, "nullifier root gate");
    let spend_proofs = build_spend_proofs(
        &tree,
        &state_tree,
        &nf_tree,
        &commitments,
        utxo_root,
        nullifier_root,
        root_index,
    );

    let spp_tx_hashes = SppTxHashes::new(&spp_proof_inputs).expect("spp tx hashes");
    let escrow_proof_inputs = EscrowProofInputParams {
        escrow_utxo: escrow_utxo.clone(),
        change,
        spp_tx_hashes,
    };

    let prover = ProverClient::local();
    let escrow_prover = EscrowProverClient::new();
    let (transact, spp_dur) = prove_transact_warm(
        spp_proof_inputs,
        &spend_proofs,
        &prover,
        &[&creator.nullifier_key],
    );
    let escrow_proof_inputs = escrow_proof_inputs
        .to_proof_inputs()
        .expect("escrow proof inputs");
    assert_eq!(
        escrow_proof_inputs.private_tx_hash, transact.private_tx_hash,
        "the escrow proof binds the SPP transaction"
    );
    let (escrow_proof, circuit) = measure_circuit(CircuitId::Escrow, || {
        escrow_prover
            .prove_escrow(&escrow_proof_inputs)
            .expect("escrow prove")
    });

    let ix = Escrow {
        creator: payer.pubkey(),
        payer: payer.pubkey(),
        tree,
        escrow_proof: escrow_proof.into(),
        spp_proof: transact,
    }
    .instruction()
    .expect("escrow instruction");

    let fixtures = vec![
        (tree, tree_account),
        (payer.pubkey(), system_owned_account(100_000_000_000)),
    ];
    bench.record("escrow", circuit, &ix, &fixtures, spp_dur);
}

fn bench_withdraw(bench: &mut Bench) {
    const LOCK_AMOUNT: u64 = 400_000;
    const UNLOCK_TIMESTAMP: u64 = 1_000_000;
    const SPP_RELAYER_DEADLINE: u64 = 2_000_000_000;

    let tree = spr_interface::pda::tree(BENCH_TREE_ID);
    let payer = Keypair::new_from_array([WITHDRAW_CREATOR_SEED; 32]);
    let creator = keypair_from_payer(&payer);
    let creator_address = creator.shielded_address().expect("creator address");

    // The spent escrow UTXO.
    let escrow_utxo = EscrowUtxo {
        terms: EscrowTerms {
            creator: creator_address,
            unlock_timestamp: UNLOCK_TIMESTAMP,
        },
        blinding: INPUT_BLINDING,
        asset: spr_transaction::Mint::SOL,
        amount: LOCK_AMOUNT,
    };
    let source_output = escrow_utxo.source_output(creator_address, random_blinding());

    let escrow_input_utxo = escrow_utxo
        .to_input_utxo(BENCH_TREE_ID, 0)
        .expect("escrow input_utxo");
    let input_utxos = vec![escrow_input_utxo];
    let mut withdraw_outputs = vec![
        source_output,
        SppProofOutputUtxo {
            compact: true,
            ..Default::default()
        },
    ];
    let blinding_seed = prepare_output_blindings(&input_utxos, &mut withdraw_outputs)
        .expect("derive withdraw output blinding");
    let [source_output, compact_output]: [_; 2] = withdraw_outputs
        .try_into()
        .expect("withdraw transaction has two output slots");

    let payer_address = Address::new_from_array(payer.pubkey().to_bytes());
    let transaction_viewing_key = get_transaction_viewing_key(&creator, &input_utxos)
        .expect("withdraw transaction viewing key");
    let encoded = encrypt_transaction_data(
        std::slice::from_ref(&source_output),
        &transaction_viewing_key,
        BENCH_TREE_ID,
    )
    .expect("encode withdraw slots");

    let mut external_data = ExternalData::new(
        *transaction_viewing_key.pubkey().as_bytes(),
        encoded.salt,
        encoded.outputs,
        encoded.resolved_owner_tags,
        vec![],
    );
    external_data.expiry_unix_ts = SPP_RELAYER_DEADLINE;
    let spp_proof_inputs = SppProofInputs {
        input_utxos,
        output_utxos: [encoded.output_utxos, vec![compact_output]].concat(),
        external_data,
        payer: payer_address,
        blinding_seed,
        output_tree_id: BENCH_TREE_ID,
        cache_accounts: Default::default(),
    };

    let commitments = spp_proof_inputs
        .input_utxo_hashes()
        .expect("input commitments");
    let leaves: Vec<[u8; 32]> = commitments.iter().map(|input| input.utxo_hash).collect();
    let (tree_account, utxo_root, nullifier_root, root_index) = build_tree_fixture(&tree, &leaves);
    let state_tree = local_state_tree(&leaves);
    assert_eq!(state_tree.root(), utxo_root, "state root gate");
    let nf_tree = nullifier_tree();
    assert_eq!(nf_tree.root(), nullifier_root, "nullifier root gate");
    let spend_proofs = build_spend_proofs(
        &tree,
        &state_tree,
        &nf_tree,
        &commitments,
        utxo_root,
        nullifier_root,
        root_index,
    );

    let withdraw_proof_inputs = WithdrawProofInputParams {
        escrow_utxo: escrow_utxo.clone(),
        source_output,
        private_tx_blinding: spp_proof_inputs
            .private_tx_blinding()
            .expect("private tx blinding"),
        input_tree_id: BENCH_TREE_ID,
        output_tree_id: BENCH_TREE_ID,
    };

    let prover = ProverClient::local();
    let escrow_prover = EscrowProverClient::new();
    let (transact, spp_dur) = prove_transact_warm(
        spp_proof_inputs,
        &spend_proofs,
        &prover,
        &[&spr_keypair::NullifierKey::from_secret([0; 31])],
    );
    let withdraw_proof_inputs = withdraw_proof_inputs
        .to_proof_inputs()
        .expect("withdraw proof inputs");
    let (withdraw_proof, circuit) = measure_circuit(CircuitId::Withdraw, || {
        escrow_prover
            .prove_withdraw(&withdraw_proof_inputs)
            .expect("withdraw prove")
    });

    let ix = Withdraw {
        creator: creator_address
            .solana_address()
            .expect("creator solana address"),
        payer: payer.pubkey(),
        tree,
        withdraw_proof: withdraw_proof.into(),
        unlock_timestamp: UNLOCK_TIMESTAMP,
        spp_proof: transact,
    }
    .instruction()
    .expect("withdraw instruction");

    bench.mollusk.sysvars.clock.unix_timestamp = UNLOCK_TIMESTAMP as i64 + 1;

    let fixtures = vec![
        (tree, tree_account),
        (payer.pubkey(), system_owned_account(100_000_000_000)),
    ];
    bench.record("withdraw", circuit, &ix, &fixtures, spp_dur);
}
