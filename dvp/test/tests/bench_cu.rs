//! CU benchmark of the five DvP instructions, replayed under mollusk.
//!
//! The profiled DvP program calls a profiler syscall that a validator does
//! not register, so it is built into its own directory, never
//! `target/deploy` (the e2e tests deploy from there). From the repository
//! root:
//!
//! ```sh
//! cargo build-sbf --manifest-path dvp/program/Cargo.toml --tools-version v1.54 \
//!     --sbf-out-dir dvp/target/bench -- --features bpf-entrypoint,profile-program
//! SHIELDED_POOL_PROGRAM_PATH=/path/to/shielded_pool_program.so \
//! ZOLANA_CLI_BIN=/path/to/zolana \
//!     cargo test --release --locked --manifest-path dvp/Cargo.toml -p dvp-test --test bench_cu -- --ignored --nocapture
//! ```
//!
//! - The shielded pool program is read
//!   from `SHIELDED_POOL_PROGRAM_PATH`, else from
//!   `dvp/target/bench/shielded_pool_program.so`.
//! - The SPP proofs come from the local prover server: one already serving
//!   `ZOLANA_PROVER_URL` is reused, else the `zolana` CLI
//!   (`ZOLANA_CLI_BIN`) starts one with the keys in `ZOLANA_PROVER_KEYS_DIR`,
//!   else `~/.config/zolana/proving-keys`.
//! - The DvP circuit proofs come from the program prover with the
//!   keys in `dvp/target/zk/dvp-program`.
//! - Circuit constraint counts come from the compiled program circuits.
//!
//! The SPP tree account is built directly: the program's tree init, then the
//! input UTXO hashes appended. The SPP proofs prove against local copies of
//! the state tree and the (empty) nullifier tree, whose roots the tree
//! account holds.
use std::{
    env, fs,
    path::{Path, PathBuf},
    sync::Mutex,
    time::{Duration, Instant},
};

use dvp_sdk::proof::CircuitId;
use dvp_sdk::{
    instructions::{
        abort_dvp::{AbortDvp, AbortDvpTransactionParams, AbortMode},
        cancel_dvp::CancelDvp,
        create_dvp::{CreateDvpDerivation, CreateDvpTransactionParams},
        reclaim_dvp::{ReclaimDvp, ReclaimDvpTransactionParams},
        reject_dvp::RejectDvp,
        settle_dvp::{SettleDvp, SettleDvpTransactionParams},
    },
    state::{Deposit, DvpUtxo, Leg, SwapDvp},
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
use zolana_client::{
    transaction_size, ClientError, ComputeBudgetConfig, MerkleContext, MerkleProof,
    NonInclusionProof, Proof, ProveRequest, Prover, ProverClient, ProverExt, SpendProof,
    STATE_TREE_HEIGHT,
};
use zolana_hasher::Poseidon;
use zolana_interface::{
    instruction::instruction_data::transact::TransactIxData,
    state::{
        default_tree_fees, discriminator::TREE_ACCOUNT_DISCRIMINATOR, nullifier_tree_params,
        tree_account_size, STATE_HEIGHT,
    },
    SHIELDED_POOL_PROGRAM_ID,
};
use zolana_keypair::{random_blinding, NullifierKey, ShieldedKeypair};
use zolana_merkle_tree::{indexed::IndexedMerkleTree, MerkleTree};
use zolana_test_utils::{transact::nullifier_tree, utxo::dummy_proofs};
use zolana_transaction::{instructions::transact::SppProofInputs, Mint, SOL_MINT};
use zolana_tree::TreeAccount;

const PROFILING_SBF_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../target/bench");
const OUTPUT_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../BENCHMARK.md");
/// The whole per-transaction budget, which every instruction must fit.
const TRANSACT_CU_LIMIT: u32 = 1_400_000;
/// The version 1 transaction size limit.
const V1_TX_LIMIT: usize = 4096;

/// Raw id of the fixture tree. `build_tree_fixture` initializes the account
/// with this id, and every UTXO commitment folds it in. It is also
/// `DVP_ADDRESS_TREE_ID`, the tree the create is pinned to.
const BENCH_TREE_ID: u16 = 0;
/// The mollusk slot, past every fixture leaf's append slot.
const BENCH_SLOT: u64 = 1_000;
/// The mollusk cluster time; the terms expire an hour later.
const NOW: i64 = 1_700_000_000;
const DELIVERY_AMOUNT: u64 = 400_000_000;
const PAYMENT_AMOUNT: u64 = 250_000_000;
/// What the deposits hold above their leg amount; settle returns it as change.
const SURPLUS: u64 = 50_000_000;
/// Pool asset id of the delivery mint. No instruction moves public funds, so
/// the mint needs no registration.
const DELIVERY_ASSET_ID: u64 = 2;

#[test]
#[ignore = "CU benchmark; slow, needs the profiling SBF build and a prover. See the module docs"]
/// 1. Configure the program runtime and fixture clock.
/// 2. Start the prover and check all program keys.
/// 3. Measure every instruction and write the report.
fn bench_cu_dvp() {
    // 1. Configure the program runtime and fixture clock.
    env::set_var("SBF_OUT_DIR", PROFILING_SBF_DIR);

    let dvp_id = dvp_program::ID;
    let spp_id = Address::new_from_array(SHIELDED_POOL_PROGRAM_ID);

    let mut mollusk = Mollusk::default();
    mollusk.compute_budget.compute_unit_limit = u64::from(TRANSACT_CU_LIMIT);
    // Keep the runtime slot after every fixture append.
    mollusk.warp_to_slot(BENCH_SLOT);
    mollusk.sysvars.clock.unix_timestamp = NOW;
    register_profiling_syscalls(&mut mollusk);
    mollusk.add_program(&dvp_id, "dvp_program");
    mollusk.add_program_with_loader_and_elf(&spp_id, &LOADER_V3, &shielded_pool_elf());

    let report = CuBenchmark::new(ReadmeConfig {
        title: "Private DvP -- CU Benchmark".into(),
        description: "1. Measure the five DvP instructions under Mollusk using the v0.5.0-alpha shielded-pool program and the Xark-backed SDK. Each DvP handler verifies its generated program proof and invokes SPP directly. Cancellation and rejection share process_abort_dvp.\n\n2. Record SPP and program proving times, consumed compute units, and serialized v1 transaction sizes. Create/reclaim use 1x2 with compact output padding; settle uses 3x4; cancel uses 3x2 with both deposits funded; reject uses 2x2 with only the payment deposit. Absent deposits are omitted from the SPP transaction.\n\n3. Report compiled program constraints, checksummed Xark proving-key and verifying-key sizes. Native proof times are single samples in the release profile, including artifact checksum checks, circuit/key loading, proving and verification. Build the profiling program first with cargo build-sbf --manifest-path dvp/program/Cargo.toml --tools-version v1.54 --sbf-out-dir dvp/target/bench -- --features bpf-entrypoint,profile-program.".into(),
        output_path: OUTPUT_PATH.into(),
        regenerate_command: Some(
            "cargo test --release --locked -p dvp-test --test bench_cu -- --ignored --nocapture".into(),
        ),
        ..Default::default()
    });

    // 2. Start the prover and check all program keys.
    start_prover();
    for circuit in CircuitId::ALL {
        dvp_sdk::proof::verifying_key(circuit).expect("preload DvP keys");
    }

    let mut bench = Bench {
        mollusk,
        tree: zolana_interface::pda::tree(BENCH_TREE_ID),
        prover: ProverClient::local(),
        report,
        circuit_proof_times: Vec::new(),
    };
    // 3. Measure every instruction and write the report.
    bench_create_dvp(&mut bench);
    bench_reclaim_dvp(&mut bench);
    bench_settle_dvp(&mut bench);
    bench_abort_dvp(&mut bench, AbortMode::Cancel);
    bench_abort_dvp(&mut bench, AbortMode::Reject);
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
            default_tree_fees(nullifier_tree_params().input_queue_zkp_batch_size)
                .expect("default tree fees"),
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
fn prove_spp(
    bench: &Bench,
    proof_inputs: SppProofInputs,
    private_tx_hash: [u8; 32],
) -> (Account, TransactIxData, Duration) {
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

/// Measure SPP request time separately from the create program proof.
struct TimedProver {
    inner: ProverClient,
    elapsed: Mutex<Duration>,
}

impl TimedProver {
    fn take(&self) -> Duration {
        std::mem::take(&mut *self.elapsed.lock().expect("timer lock"))
    }
}

impl Prover for TimedProver {
    fn prove(&self, request: &dyn ProveRequest) -> Result<Proof, ClientError> {
        let start = Instant::now();
        let proof = self.inner.prove(request);
        *self.elapsed.lock().expect("timer lock") += start.elapsed();
        proof
    }
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
    zolana_client::spawn_prover().expect("spawn prover");
}

/// The release shielded pool program: `SHIELDED_POOL_PROGRAM_PATH`, else
/// `shielded_pool_program.so` next to the profiled DvP program.
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
            "SPP proof".into(),
            "DvP circuit proof".into(),
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

/// Each DvP circuit's constraint count, key sizes and the circuit proof time
/// of the first instruction proving it (cancel_dvp for abort_dvp).
fn circuits_table(proof_times: &[(CircuitId, Duration)]) -> SectionTable {
    let rows = CircuitId::ALL
        .iter()
        .map(|&circuit| {
            let name = circuit.name();
            let count = dvp_sdk::proof::constraint_count(circuit)
                .expect("constraint count")
                .to_string();
            let keys_dir = dvp_sdk::proof::keys_dir();
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
            "Proof time".into(),
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

fn pubkey(keypair: &ShieldedKeypair) -> Address {
    keypair
        .shielded_address()
        .expect("shielded address")
        .solana_address()
        .expect("solana address")
}

/// The seller (also the creator), the buyer, the settlement authority, the
/// fee payer and their terms.
struct Parties {
    payer: Keypair,
    seller: ShieldedKeypair,
    buyer: ShieldedKeypair,
    settlement_authority: ShieldedKeypair,
    terms: SwapDvp,
}

impl Parties {
    /// Build signer fixtures and private SPL/SOL terms.
    fn new() -> Self {
        let shielded = || ShieldedKeypair::from_keypair(&Keypair::new()).expect("shielded keypair");
        let (seller, buyer, settlement_authority) = (shielded(), shielded(), shielded());
        let seller_address = seller.shielded_address().expect("seller address");
        let buyer_address = buyer.shielded_address().expect("buyer address");
        let terms = SwapDvp {
            seller: pubkey(&seller),
            buyer: pubkey(&buyer),
            delivery_mint: Address::new_from_array([3; 32]),
            payment_mint: SOL_MINT,
            settlement_authority: pubkey(&settlement_authority),
            delivery_amount: DELIVERY_AMOUNT,
            payment_amount: PAYMENT_AMOUNT,
            expiry_timestamp: NOW + 3_600,
            nonce: 1,
            ref_string: [0; 64],
            seller_settlement_destination: seller_address,
            buyer_settlement_destination: buyer_address,
            earliest_settlement_timestamp: None,
            seller_refund_destination: seller_address,
            buyer_refund_destination: buyer_address,
            creator: pubkey(&seller),
            blinding: random_blinding(),
        };
        Self {
            payer: Keypair::new(),
            seller,
            buyer,
            settlement_authority,
            terms,
        }
    }
}

fn delivery_mint(terms: &SwapDvp) -> Mint {
    Mint::new(terms.delivery_mint, DELIVERY_ASSET_ID)
}

/// A deposit funding `leg` in full plus [`SURPLUS`], at `leaf_index` of the
/// fixture tree.
fn deposit(terms: &SwapDvp, leg: Leg, leaf_index: u64) -> Deposit {
    let mint = match leg {
        Leg::Delivery => delivery_mint(terms),
        Leg::Payment => Mint::SOL,
    };
    Deposit {
        terms: terms.clone(),
        leg,
        mint,
        amount: leg.amount(terms) + SURPLUS,
        blinding: random_blinding(),
        tree_id: BENCH_TREE_ID,
        leaf_index,
    }
}

struct Bench {
    mollusk: Mollusk,
    tree: Address,
    prover: ProverClient,
    report: CuBenchmark,
    /// The circuit proof time of the first instruction proving each circuit.
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
        if !self
            .circuit_proof_times
            .iter()
            .any(|(proven, _)| *proven == circuit)
        {
            self.circuit_proof_times.push((circuit, circuit_dur));
        }
    }
}

fn bench_create_dvp(bench: &mut Bench) {
    let parties = Parties::new();
    let terms = &parties.terms;
    let address = CreateDvpDerivation::new(terms, BENCH_TREE_ID)
        .expect("derivation")
        .address;
    // 1. Prove the reserved address is absent from the nullifier tree.
    let (tree_account, _, nullifier_root, _) = build_tree_fixture(&bench.tree, &[]);
    let nf_tree = nullifier_tree().expect("nullifier tree");
    assert_eq!(nf_tree.root(), nullifier_root, "nullifier root gate");
    let params = || CreateDvpTransactionParams {
        terms: terms.clone(),
        payer: parties.payer.pubkey(),
        recipients: [
            &parties.seller,
            &parties.buyer,
            &parties.settlement_authority,
        ]
        .map(|party| party.shielded_address().expect("recipient address")),
        tree_id: BENCH_TREE_ID,
        non_inclusion: non_inclusion_proof(&bench.tree, &nf_tree, address),
    };

    let prover = TimedProver {
        inner: ProverClient::local(),
        elapsed: Mutex::new(Duration::ZERO),
    };
    // 2. Warm the SPP prover before measuring.
    params()
        .build()
        .expect("build create")
        .prove(&prover, bench.tree)
        .expect("warm prove create");
    prover.take();
    let start = Instant::now();
    let create = params()
        .build()
        .expect("build create")
        .prove(&prover, bench.tree)
        .expect("prove create");
    let total = start.elapsed();
    let spp = prover.take();
    let ix = create.instruction().expect("create_dvp instruction");

    bench.record(
        "create_dvp",
        CircuitId::CreateDvp,
        &ix,
        &parties.payer.pubkey(),
        tree_account,
        (spp, total.saturating_sub(spp)),
    );
}

fn bench_reclaim_dvp(bench: &mut Bench) {
    let parties = Parties::new();
    let terms = &parties.terms;
    let transaction = ReclaimDvpTransactionParams {
        deposit: deposit(terms, Leg::Delivery, 0),
        payer: parties.payer.pubkey(),
        refund_destination: None,
        tree_id: BENCH_TREE_ID,
    }
    .build(&parties.seller)
    .expect("build reclaim");
    let proof_inputs = transaction.proof_inputs;
    let (tree_account, spp, spp_dur) = prove_spp(
        bench,
        transaction.spp_proof_inputs.clone(),
        transaction
            .spp_proof_inputs
            .padding_independent_private_tx_hash()
            .expect("private transaction hash"),
    );
    let start = Instant::now();
    let reclaim_proof =
        dvp_sdk::proof::prove(CircuitId::ReclaimDvp, &proof_inputs).expect("reclaim prove");
    let circuit_dur = start.elapsed();
    let (_, bump) = terms.dvp_authority().expect("dvp authority");

    let ix = ReclaimDvp {
        payer: parties.payer.pubkey(),
        signer: pubkey(&parties.seller),
        tree: bench.tree,
        creator: terms.creator,
        dvp_id: terms.dvp_id().expect("DvP id"),
        bump,
        reclaim_proof,
        spp,
    }
    .instruction()
    .expect("reclaim_dvp instruction");

    bench.record(
        "reclaim_dvp",
        CircuitId::ReclaimDvp,
        &ix,
        &parties.payer.pubkey(),
        tree_account,
        (spp_dur, circuit_dur),
    );
}

fn bench_settle_dvp(bench: &mut Bench) {
    let parties = Parties::new();
    let terms = &parties.terms;
    // 1. Choose a recent timestamp inside TIME_WINDOW.
    let current_timestamp = NOW - 2;
    let transaction = SettleDvpTransactionParams {
        terms: terms.clone(),
        current_timestamp,
        dvp_utxo: DvpUtxo::new(terms.clone(), BENCH_TREE_ID),
        dvp_utxo_leaf_index: 0,
        delivery_deposit: deposit(terms, Leg::Delivery, 1),
        payment_deposit: deposit(terms, Leg::Payment, 2),
        payer: parties.payer.pubkey(),
        tree_id: BENCH_TREE_ID,
    }
    .build(&parties.settlement_authority)
    .expect("build settle");
    let proof_inputs = transaction.proof_inputs;
    let (tree_account, spp, spp_dur) = prove_spp(
        bench,
        transaction.spp_proof_inputs.clone(),
        transaction
            .spp_proof_inputs
            .padding_independent_private_tx_hash()
            .expect("private transaction hash"),
    );
    let start = Instant::now();
    let settle_proof =
        dvp_sdk::proof::prove(CircuitId::SettleDvp, &proof_inputs).expect("settle prove");
    let circuit_dur = start.elapsed();
    let (_, bump) = terms.dvp_authority().expect("dvp authority");

    let ix = SettleDvp {
        payer: parties.payer.pubkey(),
        settlement_authority: pubkey(&parties.settlement_authority),
        tree: bench.tree,
        creator: terms.creator,
        dvp_id: terms.dvp_id().expect("DvP id"),
        bump,
        current_timestamp,
        settle_proof,
        spp,
    }
    .instruction()
    .expect("settle_dvp instruction");

    bench.record(
        "settle_dvp",
        CircuitId::SettleDvp,
        &ix,
        &parties.payer.pubkey(),
        tree_account,
        (spp_dur, circuit_dur),
    );
}

/// Cancel by the settlement authority with both legs funded, or reject by
/// the buyer with payment only, omitting the absent delivery deposit.
fn bench_abort_dvp(bench: &mut Bench, mode: AbortMode) {
    let parties = Parties::new();
    let terms = &parties.terms;
    let (name, signer, delivery_deposit, payment_deposit) = match mode {
        AbortMode::Cancel => (
            "cancel_dvp",
            &parties.settlement_authority,
            Some(deposit(terms, Leg::Delivery, 1)),
            Some(deposit(terms, Leg::Payment, 2)),
        ),
        AbortMode::Reject => (
            "reject_dvp",
            &parties.buyer,
            None,
            Some(deposit(terms, Leg::Payment, 1)),
        ),
    };
    let transaction = AbortDvpTransactionParams {
        terms: terms.clone(),
        mode,
        dvp_utxo: DvpUtxo::new(terms.clone(), BENCH_TREE_ID),
        dvp_utxo_leaf_index: 0,
        delivery_mint: delivery_mint(terms),
        payment_mint: Mint::SOL,
        delivery_deposit,
        payment_deposit,
        payer: parties.payer.pubkey(),
        tree_id: BENCH_TREE_ID,
    }
    .build(signer)
    .expect("build abort");
    let proof_inputs = transaction.proof_inputs;
    let (tree_account, spp, spp_dur) = prove_spp(
        bench,
        transaction.spp_proof_inputs.clone(),
        transaction
            .spp_proof_inputs
            .padding_independent_private_tx_hash()
            .expect("private transaction hash"),
    );
    let start = Instant::now();
    let abort_proof =
        dvp_sdk::proof::prove(CircuitId::AbortDvp, &proof_inputs).expect("abort prove");
    let circuit_dur = start.elapsed();
    let (_, bump) = terms.dvp_authority().expect("dvp authority");

    let payer = parties.payer.pubkey();
    let abort = match mode {
        AbortMode::Cancel => AbortDvp::Cancel(CancelDvp {
            payer,
            settlement_authority: pubkey(signer),
            tree: bench.tree,
            creator: terms.creator,
            dvp_id: terms.dvp_id().expect("DvP id"),
            bump,
            cancel_proof: abort_proof,
            spp,
        }),
        AbortMode::Reject => AbortDvp::Reject(RejectDvp {
            payer,
            signer: pubkey(signer),
            tree: bench.tree,
            creator: terms.creator,
            dvp_id: terms.dvp_id().expect("DvP id"),
            bump,
            reject_proof: abort_proof,
            spp,
        }),
    };
    let ix = abort.instruction().expect("abort instruction");

    bench.record(
        name,
        CircuitId::AbortDvp,
        &ix,
        &payer,
        tree_account,
        (spp_dur, circuit_dur),
    );
}
