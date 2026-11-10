//! The Xark half of the shared benchmark's proving split, for the two Xark
//! `test/tests/bench_cu.rs` benches through
//! `#[path = "../../../../test-utils/xark_proving.rs"] mod xark_proving;`
//! next to `bench_report`.
//!
//! Xark 0.3.0's `Circuit::prove` (`prove_with_key`) redoes on every call what
//! a prover could keep in memory: it re-reads `circuit.xbc`, rebuilds the
//! constraint rows with `rows_for_key`, opens `pk.bin` and decodes the key's
//! points from the file while proving. [`per_proof_setup`] times the reads and
//! the row rebuild on their own. [`in_memory_proof`] times a proof with the
//! rows and the fully decoded key built once, through the same public
//! pipeline `prove_with_key` runs: resolve the named inputs, solve the
//! witness, lay it out over the rows and prove.
use std::{collections::BTreeMap, path::Path, time::Duration};

use xark::prover::{lower_witness, resolve_input_ids, rows_for_key, Fr};
use xark_groth16::{
    pk::G1Query, ConstraintSource, InMemoryPk, PkSource, ProverConfig, ProvingKey, SerializedPk,
};

use crate::bench_report::warm_median;

/// Points per chunk when decoding the proving key into memory.
const KEY_DECODE_CHUNK: usize = 1 << 16;

/// The [`warm_median`] of the work `Circuit::prove` redoes before proving on
/// each call for the circuit in `dir`: read `circuit.xbc`, open `pk.bin`,
/// look for an `r1cs.stream` row cache and build the constraint rows with
/// `rows_for_key`, in `prove_with_key`'s order.
pub fn per_proof_setup(dir: &Path) -> Duration {
    let (rows, setup) = warm_median(|| {
        let xbc = read(&dir.join("circuit.xbc"));
        let pk_path = dir.join("pk.bin");
        let key = SerializedPk::open(&pk_path)
            .unwrap_or_else(|e| panic!("open {}: {e}", pk_path.display()));
        let cached = std::fs::read(dir.join("r1cs.stream")).ok();
        rows_for_key(&xbc, key.header().a_len, cached).expect("constraint rows")
    });
    std::hint::black_box(rows);
    setup
}

/// The [`warm_median`] of an in-memory proof of `inputs` (input leaf name to
/// decimal value) for the circuit in `dir`. The primitive program, the
/// constraint rows and the decoded proving key are built once before timing.
/// Each timed proof resolves the inputs, solves and checks the witness and
/// proves; the last proof must verify under the key's verifying key.
///
/// The proofs use fixed Groth16 randomness `r` and `s` so the bench needs no
/// RNG. That is for timing only: a proof with known `r` and `s` is not
/// zero-knowledge.
pub fn in_memory_proof(dir: &Path, inputs: &BTreeMap<String, String>) -> Duration {
    let xbc = read(&dir.join("circuit.xbc"));
    let program = xark_ir::function_decode::expand_function_blob(&xbc)
        .expect("expand circuit.xbc")
        .to_primitive();
    // The whole proving key, every point vector decoded.
    let pk_bytes = read(&dir.join("pk.bin"));
    let mut serialized = SerializedPk::from_bytes(&pk_bytes).expect("parse pk.bin");
    let [a_query, b_g1_query, h_query, l_query] =
        [G1Query::A, G1Query::BG1, G1Query::H, G1Query::L].map(|query| {
            let mut points = Vec::new();
            serialized
                .stream_g1(query, KEY_DECODE_CHUNK, &mut |_, chunk| {
                    points.extend_from_slice(chunk)
                })
                .expect("decode proving key G1 points");
            points
        });
    let mut b_g2_query = Vec::new();
    serialized
        .stream_g2(KEY_DECODE_CHUNK, &mut |_, chunk| {
            b_g2_query.extend_from_slice(chunk)
        })
        .expect("decode proving key G2 points");
    let header = serialized.header();
    let key = ProvingKey {
        vk: header.vk.clone(),
        beta_g1: header.beta_g1,
        delta_g1: header.delta_g1,
        a_query,
        b_g1_query,
        b_g2_query,
        h_query,
        l_query,
    };
    let rows = rows_for_key(&xbc, key.a_query.len(), None).expect("constraint rows");

    let ((proof, public), proving) = warm_median(|| {
        let ids = resolve_input_ids(&program.vars, inputs).expect("resolve circuit inputs");
        let solved =
            xark_ir::solver::solve_and_check(&program, &ids).expect("inputs satisfy the circuit");
        let witness = lower_witness(&solved);
        let assignment = rows.layout().assignment(|id| witness.get(&id).copied());
        let proof = xark_groth16::prove_with_randomness(
            &*rows,
            &assignment,
            &mut InMemoryPk::new(&key),
            Fr::from(7u64),
            Fr::from(11u64),
            &ProverConfig::default(),
            &mut (),
        )
        .expect("in-memory proof");
        let public = assignment
            .get(1..rows.layout().num_instance())
            .expect("public inputs")
            .to_vec();
        (proof, public)
    });
    assert_eq!(
        xark_groth16::verify(&key.vk, &public, &proof),
        Ok(true),
        "in-memory proof verifies"
    );
    proving
}

fn read(path: &Path) -> Vec<u8> {
    std::fs::read(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}
