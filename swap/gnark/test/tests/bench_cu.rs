//! CU and proving benchmark of the swap make, take and cancel instructions.
//! It writes `../BENCHMARK.md` (see `test-utils/compare-benchmarks.py` for
//! the gnark/Xark comparison). From `swap/gnark`:
//!
//! 1. Generate the circuit keys into `build/gnark/<circuit>` with
//!    `cargo run --release --manifest-path prover/Cargo.toml --bin swap-prover-setup --
//!    <circuit> build/gnark/<circuit> --insecure-test-keys`.
//! 2. Build the profiling program:
//!    `cargo build-sbf --manifest-path program/Cargo.toml --tools-version v1.54
//!    --sbf-out-dir target/bench -- --features bpf-entrypoint,profile-program`.
//! 3. Export the variables the `cli-v0.10.0` release CLI prints with
//!    `zolana dev artifacts` (see `test-utils/run-release-tests.sh`), and copy
//!    the deployed shielded-pool program it names into the same directory:
//!    `cp "$SHIELDED_POOL_PROGRAM_PATH" target/bench/shielded_pool_program.so`.
//! 4. With `ZOLANA_CLI_BIN` and `PROVER_BIN` from step 3 (`ZOLANA_PROVER_URL`
//!    moves the prover's port, and a prover already serving it is reused), run
//!    `cargo test --release --manifest-path test/Cargo.toml --test bench_cu --
//!    --ignored --nocapture`.
//!
//! The swap circuit proofs come from the in-process gnark prover with the
//! keys in `build/gnark`. Each cold key load runs in a child process of the
//! bench (`key_load_probe`).
#[path = "../../../../test-utils/bench_report.rs"]
mod bench_report;
#[path = "../../../../test-utils/gnark_circuit_size.rs"]
mod gnark_circuit_size;

mod utxo;

use spr_transaction::utxo::SppProofInputUtxo;
use std::{env, path::Path, time::Duration};

use bench_report::{
    cold_key_load, proof_bytes, time_key_load, warm_median, CircuitRow, Implementation,
    InstructionRow, Report, INPUT_BLINDING,
};
use gnark_circuit_size::circuit_size;
use light_program_profiler::mollusk::{register_profiling_syscalls, take_profiling_entries};
use mollusk_svm::{result::Check, Mollusk};
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
use spr_transaction::{
    instructions::transact::{ExternalData, SppProofInputs, SppProofOutputUtxo, BN254_MODULUS_DEC},
    Data, Utxo, SOL_ASSET_ID, SOL_MINT,
};
use spr_tree::TreeAccount;
use swap_prover::{CircuitId, OrderProof, PROVER};
use swap_sdk::{
    instructions::{
        cancel::{Cancel, CancelProofInputParams},
        make::{Make, MakeProofInputParams, OrderMarker, SppTxHashes},
        take::{take_blinding_seed, Take, TakeProofInputParams},
    },
    prover::SwapProverClient,
    shared::input_sum,
    state::{OrderTerms, OrderUtxo},
};
use utxo::{
    assign_output_blindings, encrypt_transaction_data, get_transaction_viewing_key,
    prepare_output_blindings, ProofKeys,
};

/// Holds the profiling `swap_program.so` and a plain `shielded_pool_program.so`.
const BENCH_SBF_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../target/bench");
const OUTPUT_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../BENCHMARK.md");

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
/// The runtime slot, after the slots `build_tree_fixture` appends leaves at.
const BENCH_SLOT: u64 = 1_000;
/// Keypair seeds of each instruction's paying party; the counterparties use
/// fixed keys too. Fixed keys make every run replay the same transactions.
const MAKE_MAKER_SEED: u8 = 1;
const TAKE_TAKER_SEED: u8 = 3;
const CANCEL_MAKER_SEED: u8 = 5;
/// Blinding of the dummy input, whose nullifier the SPP records too.
const DUMMY_BLINDING: [u8; 32] = bench_report::fixed_blinding(6);

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

/// `utxo` as an input owned by `key`, committed at `leaf_index` of the
/// fixture tree with no data or ring data hash.
fn indexed_input(
    utxo: Utxo,
    key: &spr_keypair::NullifierKey,
    leaf_index: u64,
) -> SppProofInputUtxo {
    let nullifier_pubkey = key.pubkey().expect("nullifier pubkey");
    let utxo_hash = utxo
        .hash(&nullifier_pubkey, &[0; 32], &[0; 32], BENCH_TREE_ID)
        .expect("input hash");
    let nullifier = key
        .nullifier(&utxo_hash, &utxo.blinding)
        .expect("input nullifier");
    spr_transaction::WalletUtxo {
        utxo,
        nullifier_pubkey,
        utxo_hash,
        nullifier,
        tx_viewing_key: None,
        data_hash: None,
        ring_data_hash: None,
        tree_id: BENCH_TREE_ID,
        leaf_index,
        latest_tree_id: None,
        slot: 0,
        tx_signature: Default::default(),
        slot_index: 0,
    }
    .into()
}

/// Non-inclusion witnesses of the padding inputs in a fresh nullifier tree.
fn dummy_proofs(transaction: &SppProofInputs) -> Vec<NonInclusionProof> {
    let tree = nullifier_tree();
    transaction
        .input_utxos
        .iter()
        .filter(|input| input.is_dummy())
        .map(|input| {
            let proof = tree
                .get_non_inclusion_proof(&BigUint::from_bytes_be(&input.nullifier))
                .expect("dummy non-inclusion");
            NonInclusionProof {
                leaf: input.nullifier,
                merkle_context: MerkleContext {
                    tree_type: 1,
                    tree: spr_interface::pda::tree(input.tree_id),
                },
                path: proof.merkle_proof.to_vec(),
                low_element: proof.leaf_lower_range_value,
                low_element_index: proof.leaf_index as u64,
                high_element: proof.leaf_higher_range_value,
                high_element_index: 0,
                root: tree.root(),
                root_seq: 0,
                root_index: 0,
            }
        })
        .collect()
}

/// Start the shared SPP prover, or reuse a running one, with the keys in
/// `ZOLANA_PROVER_KEYS_DIR` (default `~/.config/zolana/proving-keys`).
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

fn prove_transact_timed(
    proof_inputs: SppProofInputs,
    spend_proofs: &[SpendProof],
    prover: &ProverClient,
    keys: &[&spr_keypair::NullifierKey],
) -> (TransactIxData, Duration) {
    let dummy_proofs = dummy_proofs(&proof_inputs);
    warm_median(|| {
        prover
            .prove_transact(
                proof_inputs.clone(),
                spend_proofs,
                &dummy_proofs,
                &ProofKeys(keys),
            )
            .expect("prove transact")
    })
}

/// The version 1 transaction this instruction is sent as. Its compute ceilings
/// live in the message header.
fn v1_transaction_bytes(ix: &Instruction, payer: &Pubkey) -> u64 {
    let bytes = transaction_size(
        payer,
        std::slice::from_ref(ix),
        ComputeBudgetConfig::new(1_400_000),
    )
    .expect("measure v1 transaction")
    .bytes;
    u64::try_from(bytes).expect("fits u64")
}

/// The cold key loads of every circuit, each load in a fresh child process,
/// run before anything is loaded into this process.
struct KeyLoads(Vec<(CircuitId, Duration, Vec<String>)>);

impl KeyLoads {
    fn measure() -> Self {
        Self(
            CircuitId::ALL
                .iter()
                .map(|circuit| {
                    let (median, outputs) = cold_key_load(circuit.name());
                    (*circuit, median, outputs)
                })
                .collect(),
        )
    }

    /// Measure `circuit`: its cold key load, its warm proof time with the
    /// keys preloaded into the in-process prover, and its sizes. `prove`
    /// proves the instruction's inputs once; the last proof is returned for
    /// the instruction.
    fn measure_circuit(
        &self,
        circuit: CircuitId,
        prove: impl FnMut() -> OrderProof,
    ) -> (OrderProof, CircuitRow) {
        let (_, key_load, outputs) = self
            .0
            .iter()
            .find(|(loaded, _, _)| *loaded == circuit)
            .expect("key load of every circuit");
        let keys_dir = PROVER.keys_dir(circuit);
        let vk = std::fs::read(keys_dir.join("vk.bin")).expect("read verifying key");
        let size = circuit_size(circuit.name(), outputs, &vk);
        let (proof, warm_proof) = warm_median(prove);
        let row = CircuitRow {
            name: circuit.name().into(),
            constraints: size.constraints,
            public_inputs: size.public_inputs,
            proving_key_bytes: std::fs::metadata(keys_dir.join("pk.bin"))
                .expect("proving key")
                .len(),
            verifying_key_bytes: u64::try_from(vk.len()).expect("fits u64"),
            proof_bytes: proof_bytes(&proof.proof_a, &proof.proof_b, &proof.proof_c),
            key_load: *key_load,
            per_proof_setup: Duration::ZERO,
            warm_proof,
            in_memory_proof: warm_proof,
        };
        (proof, row)
    }
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
    fixtures: &[(Pubkey, Account)],
    warm_spp_proof: Duration,
) -> (CircuitRow, InstructionRow) {
    let accounts = assemble_accounts(ix, spp_id, fixtures);
    let mollusk_ix = to_mollusk_instruction(ix);
    mollusk.process_and_validate_instruction(&mollusk_ix, &accounts, &[Check::success()]);
    let entries = take_profiling_entries();
    assert!(!entries.is_empty(), "no profiling entries for '{name}'");
    let instruction = InstructionRow {
        name: name.into(),
        circuit: circuit.name.clone(),
        entries,
        warm_spp_proof,
        instruction_data_bytes: u64::try_from(ix.data.len()).expect("fits u64"),
        accounts: u64::try_from(ix.accounts.len()).expect("fits u64"),
        v1_tx_bytes: v1_transaction_bytes(ix, payer),
    };
    (circuit, instruction)
}

/// The child-process half of the cold key load: one preload of the circuit
/// `cold_key_load` names (compile the constraint system, read the proving key).
#[test]
#[ignore = "child process of bench_cu_swap"]
fn key_load_probe() {
    time_key_load(|name| {
        let circuit = *CircuitId::ALL
            .iter()
            .find(|circuit| circuit.name() == name)
            .expect("known circuit");
        PROVER.preload(circuit).expect("preload circuit keys");
    });
}

#[test]
#[ignore = "CU and proving benchmark; needs the SBF builds in target/bench and an SPP prover"]
fn bench_cu_swap() {
    std::env::set_var("SBF_OUT_DIR", BENCH_SBF_DIR);

    let swap_id = Pubkey::new_from_array(*swap_program::ID.as_array());
    let spp_id = Pubkey::new_from_array(SHIELDED_POOL_PROGRAM_ID);

    let mut mollusk = Mollusk::default();
    // Keep the runtime slot after every fixture append: SPP refuses a tree
    // whose last update is ahead of the clock.
    mollusk.warp_to_slot(BENCH_SLOT);
    register_profiling_syscalls(&mut mollusk);
    mollusk.add_program(&swap_id, "swap_program");
    mollusk.add_program(&spp_id, "shielded_pool_program");

    // Cold key loads first, each in a fresh child process.
    let key_loads = KeyLoads::measure();

    // Warm provers: the SPP server is up and every circuit's keys are loaded
    // in process before anything is timed.
    start_prover();
    for circuit in CircuitId::ALL {
        PROVER.preload(*circuit).expect("preload circuit keys");
    }

    let (circuits, instructions) = [
        bench_make(&mut mollusk, &spp_id, &key_loads),
        bench_take(&mut mollusk, &spp_id, &key_loads),
        bench_cancel(&mut mollusk, &spp_id, &key_loads),
    ]
    .into_iter()
    .unzip();
    Report {
        example: "Confidential Swap",
        implementation: Implementation::Gnark,
        circuits,
        instructions,
    }
    .write(OUTPUT_PATH);
}

fn bench_make(
    mollusk: &mut Mollusk,
    spp_id: &Pubkey,
    key_loads: &KeyLoads,
) -> (CircuitRow, InstructionRow) {
    const INPUT_AMOUNT: u64 = 1_000_000;
    const SOURCE_AMOUNT: u64 = 400_000;
    const EXPIRY: u64 = 1_900_000_000;

    let tree = spr_interface::pda::tree(BENCH_TREE_ID);
    let payer = Keypair::new_from_array([MAKE_MAKER_SEED; 32]);
    let maker = keypair_from_payer(&payer);

    // As in the localnet flow, the maker spends its own source and signs the
    // transact for it; the program signs for the order-authority PDA, which
    // owns the order output.
    let input_utxo = Utxo {
        owner: maker.signing_pubkey(),
        asset: spr_transaction::Mint::SOL,
        amount: INPUT_AMOUNT,
        blinding: INPUT_BLINDING,
        ring_program_id: None,
        data: Data::default(),
    };

    let taker =
        ShieldedKeypair::from_keypair(&Keypair::new_from_array([0x4d; 32])).expect("taker keypair");
    let taker_address = taker.shielded_address().expect("taker address");
    let terms = OrderTerms {
        destination_mint: Address::new_from_array([7u8; 32]),
        destination_amount: 250,
        destination: maker.shielded_address().expect("maker address"),
        taker: Some(Address::new_from_array(
            taker.signing_pubkey().as_ed25519().expect("taker pubkey"),
        )),
        expiry: EXPIRY,
    };
    let mut order_utxo = OrderUtxo {
        terms,
        blinding: random_blinding(),
        source_mint: spr_transaction::Mint::SOL,
        source_amount: SOURCE_AMOUNT,
        destination_asset_id: 2,
    };
    let order_output_utxo = order_utxo
        .output_utxo(taker_address.viewing_pubkey)
        .expect("order output");

    let payer_address = Address::new_from_array(payer.pubkey().to_bytes());
    let input_utxo = indexed_input(input_utxo, &maker.nullifier_key, 0);
    let input_utxos = vec![
        input_utxo,
        SppProofInputUtxo::dummy_with_blinding(DUMMY_BLINDING, BENCH_TREE_ID).expect("dummy"),
    ];

    let order_utxo_asset = order_output_utxo.asset;
    let leftover =
        input_sum(&input_utxos, &order_utxo_asset.asset) - i128::from(order_output_utxo.amount);
    let change_amount = u64::try_from(leftover).expect("insufficient order balance");
    let change = SppProofOutputUtxo::new(
        order_utxo_asset,
        change_amount,
        maker.shielded_address().expect("maker address"),
    )
    .expect("change output");
    let mut transaction_outputs = vec![change, order_output_utxo];
    let blinding_seed = prepare_output_blindings(&input_utxos, &mut transaction_outputs)
        .expect("derive make output blindings");
    let [change, order_output_utxo]: [_; 2] = transaction_outputs
        .try_into()
        .expect("make transaction has two outputs");
    order_utxo.blinding = order_output_utxo.blinding;

    let order_utxo_hash = order_output_utxo
        .hash(BENCH_TREE_ID)
        .expect("order output hash");
    let marker_message = OrderMarker {
        order_utxo_hash,
        maker_pubkey: payer.pubkey(),
        taker_address,
    }
    .message()
    .expect("marker message");

    let transaction_viewing_key =
        get_transaction_viewing_key(&maker, &input_utxos).expect("make transaction viewing key");

    let encoded = encrypt_transaction_data(
        &[change.clone(), order_output_utxo],
        &transaction_viewing_key,
        BENCH_TREE_ID,
    )
    .expect("encode make slots");

    let external_data = ExternalData::new(
        *transaction_viewing_key.pubkey().as_bytes(),
        encoded.salt,
        encoded.outputs,
        encoded.resolved_owner_tags,
        vec![marker_message],
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

    let make_proof_inputs = MakeProofInputParams {
        order_utxo,
        change,
        spp_tx_hashes: SppTxHashes::new(&spp_proof_inputs).expect("spp tx hashes"),
    };

    let prover = ProverClient::local();
    let swap_prover_client = SwapProverClient::new();
    let (transact, spp_dur) = prove_transact_timed(
        spp_proof_inputs,
        &spend_proofs,
        &prover,
        &[&maker.nullifier_key],
    );
    let make_inputs = make_proof_inputs
        .to_proof_inputs()
        .expect("make proof inputs");
    let (make_proof, circuit) = key_loads.measure_circuit(CircuitId::Make, || {
        swap_prover_client
            .prove_make(&make_inputs)
            .expect("swap make prove")
    });

    let ix = Make {
        payer: payer.pubkey(),
        tree,
        make_proof: make_proof.into(),
        spp_proof: transact,
    }
    .instruction()
    .expect("make instruction");

    let fixtures = vec![
        (tree, tree_account),
        (payer.pubkey(), system_owned_account(100_000_000_000)),
    ];
    replay(
        mollusk,
        spp_id,
        "make",
        circuit,
        &ix,
        &payer.pubkey(),
        &fixtures,
        spp_dur,
    )
}

fn bench_take(
    mollusk: &mut Mollusk,
    spp_id: &Pubkey,
    key_loads: &KeyLoads,
) -> (CircuitRow, InstructionRow) {
    const SOURCE_AMOUNT: u64 = 400_000;
    const DESTINATION_AMOUNT: u64 = 250;
    const EXPIRY: u64 = 1_900_000_000;

    let tree = spr_interface::pda::tree(BENCH_TREE_ID);
    let taker_payer = Keypair::new_from_array([TAKE_TAKER_SEED; 32]);
    let taker = keypair_from_payer(&taker_payer);
    let taker_address = taker.shielded_address().expect("taker address");
    let maker =
        ShieldedKeypair::from_keypair(&Keypair::new_from_array([0x51; 32])).expect("maker keypair");
    let maker_address = maker.shielded_address().expect("maker address");

    let terms = OrderTerms {
        destination_mint: SOL_MINT,
        destination_amount: DESTINATION_AMOUNT,
        destination: maker_address,
        taker: Some(Address::new_from_array(
            taker.signing_pubkey().as_ed25519().expect("taker pubkey"),
        )),
        expiry: EXPIRY,
    };
    // The spent order UTXO.
    let order_utxo = OrderUtxo {
        terms,
        blinding: INPUT_BLINDING,
        source_mint: spr_transaction::Mint::SOL,
        source_amount: SOURCE_AMOUNT,
        destination_asset_id: SOL_ASSET_ID,
    };

    // The blinding of the taker's spent payment input.
    let taker_in_blinding = INPUT_BLINDING;
    let source_output_blinding = random_blinding();

    let taker_in = order_utxo.destination_output(taker_address, taker_in_blinding);
    let source_output = order_utxo.source_output(taker_address, source_output_blinding);
    let destination_output = order_utxo.destination_output(maker_address, random_blinding());

    let order_input_utxo = order_utxo
        .to_input_utxo(BENCH_TREE_ID, 0)
        .expect("order input_utxo");
    let taker_utxo = Utxo {
        owner: taker.signing_pubkey(),
        asset: spr_transaction::Mint::SOL,
        amount: DESTINATION_AMOUNT,
        blinding: taker_in_blinding,
        ring_program_id: None,
        data: Data::default(),
    };
    let taker_input_utxo = indexed_input(taker_utxo, &taker.nullifier_key, 1);

    let payer_address = Address::new_from_array(taker_payer.pubkey().to_bytes());
    let input_utxos = vec![order_input_utxo, taker_input_utxo];
    let mut transaction_outputs = vec![source_output, destination_output];
    let blinding_seed = take_blinding_seed(&order_utxo.blinding).expect("take blinding seed");
    let first_nullifier = input_utxos.first().expect("order input").nullifier();
    let seed = spr_transaction::derive_output_blinding_seed(&first_nullifier, &blinding_seed)
        .expect("take seed");
    assign_output_blindings(&mut transaction_outputs, &first_nullifier, &seed)
        .expect("derive take output blindings");
    let [source_output, destination_output]: [_; 2] = transaction_outputs
        .try_into()
        .expect("take transaction has two outputs");
    let transaction_viewing_key =
        get_transaction_viewing_key(&taker, &input_utxos).expect("take transaction viewing key");

    let encoded = encrypt_transaction_data(
        &[source_output.clone(), destination_output.clone()],
        &transaction_viewing_key,
        BENCH_TREE_ID,
    )
    .expect("encode take slots");

    let mut external_data = ExternalData::new(
        *transaction_viewing_key.pubkey().as_bytes(),
        encoded.salt,
        encoded.outputs,
        encoded.resolved_owner_tags,
        vec![],
    );
    external_data.expiry_unix_ts = order_utxo.terms.expiry;
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

    let take_proof_inputs = TakeProofInputParams {
        order_utxo,
        taker_in,
        source_output,
        destination_output,
        private_tx_blinding: spp_proof_inputs
            .private_tx_blinding()
            .expect("private tx blinding"),
        input_tree_id: BENCH_TREE_ID,
        output_tree_id: BENCH_TREE_ID,
    };

    let prover = ProverClient::local();
    let swap_prover_client = SwapProverClient::new();
    let (transact, spp_dur) = prove_transact_timed(
        spp_proof_inputs,
        &spend_proofs,
        &prover,
        &[
            &spr_keypair::NullifierKey::from_secret([0; 31]),
            &taker.nullifier_key,
        ],
    );
    let take_inputs = take_proof_inputs
        .to_proof_inputs()
        .expect("take proof inputs");
    let (take_proof, circuit) = key_loads.measure_circuit(CircuitId::Take, || {
        swap_prover_client
            .prove_take(&take_inputs)
            .expect("swap take prove")
    });

    let ix = Take {
        payer: taker_payer.pubkey(),
        maker: maker_address
            .solana_address()
            .expect("maker solana address"),
        tree,
        take_proof: take_proof.into(),
        spp_proof: transact,
    }
    .instruction()
    .expect("take instruction");

    let fixtures = vec![
        (tree, tree_account),
        (taker_payer.pubkey(), system_owned_account(100_000_000_000)),
    ];
    replay(
        mollusk,
        spp_id,
        "take",
        circuit,
        &ix,
        &taker_payer.pubkey(),
        &fixtures,
        spp_dur,
    )
}

fn bench_cancel(
    mollusk: &mut Mollusk,
    spp_id: &Pubkey,
    key_loads: &KeyLoads,
) -> (CircuitRow, InstructionRow) {
    const SOURCE_AMOUNT: u64 = 400_000;
    const ORDER_EXPIRY: u64 = 1_000_000;
    const SPP_RELAYER_DEADLINE: u64 = u64::MAX;

    let tree = spr_interface::pda::tree(BENCH_TREE_ID);
    let maker_payer = Keypair::new_from_array([CANCEL_MAKER_SEED; 32]);
    let maker = keypair_from_payer(&maker_payer);
    let maker_address = maker.shielded_address().expect("maker address");
    let taker =
        ShieldedKeypair::from_keypair(&Keypair::new_from_array([0x4d; 32])).expect("taker keypair");
    let taker_viewing_pubkey = taker
        .shielded_address()
        .expect("taker address")
        .viewing_pubkey;
    let terms = OrderTerms {
        destination_mint: Address::new_from_array([7u8; 32]),
        destination_amount: 250,
        destination: maker_address,
        taker: Some(Address::new_from_array(
            taker.signing_pubkey().as_ed25519().expect("taker pubkey"),
        )),
        expiry: ORDER_EXPIRY,
    };
    // The spent order UTXO.
    let order_utxo = OrderUtxo {
        terms,
        blinding: INPUT_BLINDING,
        source_mint: spr_transaction::Mint::SOL,
        source_amount: SOURCE_AMOUNT,
        destination_asset_id: 2,
    };
    let source_output_blinding = random_blinding();

    let source_output = order_utxo.source_output(maker_address, source_output_blinding);

    let order_input_utxo = order_utxo
        .to_input_utxo(BENCH_TREE_ID, 0)
        .expect("order input_utxo");

    let payer_address = Address::new_from_array(maker_payer.pubkey().to_bytes());
    let input_utxos = vec![order_input_utxo];
    let mut cancel_outputs = vec![
        source_output,
        SppProofOutputUtxo {
            compact: true,
            ..Default::default()
        },
    ];
    let blinding_seed = prepare_output_blindings(&input_utxos, &mut cancel_outputs)
        .expect("derive cancel output blinding");
    let [source_output, compact_output]: [_; 2] = cancel_outputs
        .try_into()
        .expect("cancel transaction has two output slots");
    let transaction_viewing_key =
        get_transaction_viewing_key(&maker, &input_utxos).expect("cancel transaction viewing key");

    let encoded = encrypt_transaction_data(
        std::slice::from_ref(&source_output),
        &transaction_viewing_key,
        BENCH_TREE_ID,
    )
    .expect("encode cancel slots");

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

    let cancel_proof_inputs = CancelProofInputParams {
        order_utxo: order_utxo.clone(),
        taker_viewing_pubkey,
        source_output,
        private_tx_blinding: spp_proof_inputs
            .private_tx_blinding()
            .expect("private tx blinding"),
        input_tree_id: BENCH_TREE_ID,
        output_tree_id: BENCH_TREE_ID,
    };

    let prover = ProverClient::local();
    let swap_prover_client = SwapProverClient::new();
    let (transact, spp_dur) = prove_transact_timed(
        spp_proof_inputs,
        &spend_proofs,
        &prover,
        &[&spr_keypair::NullifierKey::from_secret([0; 31])],
    );
    let cancel_inputs = cancel_proof_inputs
        .to_proof_inputs()
        .expect("cancel proof inputs");
    let (cancel_proof, circuit) = key_loads.measure_circuit(CircuitId::Cancel, || {
        swap_prover_client
            .prove_cancel(&cancel_inputs)
            .expect("swap cancel prove")
    });

    let maker_signer = Pubkey::new_from_array(
        maker_address
            .signing_pubkey
            .as_ed25519()
            .expect("maker ed25519"),
    );
    let ix = Cancel {
        maker: maker_signer,
        payer: maker_payer.pubkey(),
        tree,
        cancel_proof: cancel_proof.into(),
        order_expiry: order_utxo.terms.expiry,
        spp_proof: transact,
    }
    .instruction()
    .expect("cancel instruction");

    mollusk.sysvars.clock.unix_timestamp = ORDER_EXPIRY as i64 + 1;

    let fixtures = vec![
        (tree, tree_account),
        (maker_payer.pubkey(), system_owned_account(100_000_000_000)),
    ];
    replay(
        mollusk,
        spp_id,
        "cancel",
        circuit,
        &ix,
        &maker_payer.pubkey(),
        &fixtures,
        spp_dur,
    )
}
