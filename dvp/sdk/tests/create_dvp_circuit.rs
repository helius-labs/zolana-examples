use dvp_program::{
    instructions::{
        create_dvp::CreateDvpProof,
        verifier::{verify_groth16, CompressedGroth16Proof},
    },
    verifying_keys::create_dvp::VERIFYINGKEY,
    DVP_ADDRESS_TREE_ID,
};
use dvp_prover::{CircuitId, CreateDvpProofInputs, PROVER};
use dvp_sdk::{instructions::create_dvp::CreateDvpProofInputParams, state::SwapDvp};
use groth16_solana::{
    decompression::{decompress_g1, decompress_g2},
    groth16::Groth16Verifier,
    vk::gnark::{parse_gnark_vk_bytes, Groth16VerifyingkeyOwned},
};

mod shared;
use shared::{derive_create_inputs, fe, reseal, sample_terms, OUTPUT_TREE_ID};

fn build_dir() -> std::path::PathBuf {
    PROVER.keys_dir(CircuitId::CreateDvp)
}

fn ensure_keys() {
    let dir = build_dir();
    if !dir.join("pk.bin").exists() || !dir.join("vk.bin").exists() {
        PROVER
            .setup_insecure_test_keys(CircuitId::CreateDvp, &dir)
            .expect("setup failed");
    }
}

fn generated_vk() -> Groth16VerifyingkeyOwned {
    let bytes = std::fs::read(build_dir().join("vk.bin")).expect("read vk.bin");
    parse_gnark_vk_bytes(&bytes).expect("parse vk.bin")
}

fn verify_with_generated_vk(
    vk: &Groth16VerifyingkeyOwned,
    proof_a: &[u8; 32],
    proof_b: &[u8; 64],
    proof_c: &[u8; 32],
    public_input: [u8; 32],
) -> bool {
    let (Ok(a), Ok(b), Ok(c)) = (
        decompress_g1(proof_a),
        decompress_g2(proof_b),
        decompress_g1(proof_c),
    ) else {
        return false;
    };
    let public_inputs = [public_input];
    let borrowed = vk.as_borrowed();
    match Groth16Verifier::new(&a, &b, &c, &public_inputs, &borrowed) {
        Ok(mut verifier) => verifier.verify().is_ok(),
        Err(_) => false,
    }
}

fn sdk_inputs(terms: SwapDvp) -> CreateDvpProofInputs {
    CreateDvpProofInputParams {
        terms,
        dvp_utxo_tree_id: OUTPUT_TREE_ID,
    }
    .to_proof_inputs()
    .expect("sdk proof inputs")
}

fn create_seed() -> [u8; 32] {
    sample_terms().create_blinding_seed().expect("create seed")
}

/// Tampers one field of the sample witness, reseals the hashes that follow
/// from it, and asserts the circuit refuses to prove it.
fn assert_unprovable(
    reason: &str,
    address_tree_id: u16,
    derive_blindings: bool,
    tamper: impl FnOnce(&mut CreateDvpProofInputs),
) {
    ensure_keys();
    let mut inputs = derive_create_inputs(&sample_terms());
    tamper(&mut inputs);
    reseal(
        &mut inputs,
        &create_seed(),
        address_tree_id,
        derive_blindings,
    );
    assert!(inputs.prove().is_err(), "proving must fail: {reason}");
}

/// Terms the sdk refuses: derive the witness without validation and assert
/// the circuit refuses them too.
fn assert_terms_unprovable(reason: &str, tamper: impl FnOnce(&mut SwapDvp)) {
    ensure_keys();
    let mut terms = sample_terms();
    tamper(&mut terms);
    assert!(
        CreateDvpProofInputParams {
            terms: terms.clone(),
            dvp_utxo_tree_id: OUTPUT_TREE_ID,
        }
        .to_proof_inputs()
        .is_err(),
        "the sdk must reject the terms: {reason}"
    );
    assert!(
        derive_create_inputs(&terms).prove().is_err(),
        "proving must fail: {reason}"
    );
}

#[test]
fn program_vk_has_no_commitment() {
    assert_eq!(VERIFYINGKEY.nr_pubinputs, 1);
    assert!(
        VERIFYINGKEY.vk_commitment.is_none(),
        "create_dvp circuit is standard Groth16: no BSB22 commitment"
    );
    assert_eq!(
        VERIFYINGKEY.vk_ic.len(),
        2,
        "standard Groth16 vk_ic length must be public_inputs + 1"
    );
}

/// The sdk derivation matches the independent one in `shared`, and resealing
/// an untouched witness changes nothing, so the negative tests below fail on
/// the field they tamper.
#[test]
fn sdk_inputs_match_independent_derivation() {
    let expected = derive_create_inputs(&sample_terms());
    assert_eq!(sdk_inputs(sample_terms()), expected);

    let mut resealed = expected.clone();
    reseal(&mut resealed, &create_seed(), DVP_ADDRESS_TREE_ID, true);
    assert_eq!(resealed, expected);
}

#[test]
fn create_dvp_prove_verify() {
    ensure_keys();
    let vk = generated_vk();

    let inputs = sdk_inputs(sample_terms());
    let proof = inputs.prove().expect("prove failed");
    assert!(
        proof.proof_a.iter().any(|byte| *byte != 0),
        "proof_a must not be all zero"
    );

    assert!(
        verify_with_generated_vk(
            &vk,
            &proof.proof_a,
            &proof.proof_b,
            &proof.proof_c,
            inputs.public_input_hash,
        ),
        "the proof must verify against the generated create_dvp verifying key"
    );

    let proof: CreateDvpProof = proof.into();
    verify_groth16(
        CompressedGroth16Proof {
            a: &proof.proof_a,
            b: &proof.proof_b,
            c: &proof.proof_c,
            commitment: None,
        },
        inputs.public_input_hash,
        &VERIFYINGKEY,
    )
    .expect("the committed create_dvp VERIFYINGKEY must accept the proof; regenerate the keys");
}

#[test]
fn create_dvp_without_earliest_proves() {
    ensure_keys();
    let mut terms = sample_terms();
    terms.earliest_settlement_timestamp = None;
    let inputs = sdk_inputs(terms);
    let proof = inputs.prove().expect("prove failed");
    assert!(verify_with_generated_vk(
        &generated_vk(),
        &proof.proof_a,
        &proof.proof_b,
        &proof.proof_c,
        inputs.public_input_hash,
    ));
}

#[test]
fn create_dvp_rejects_tampered_public_input() {
    ensure_keys();
    let vk = generated_vk();

    let inputs = sdk_inputs(sample_terms());
    let proof = inputs.prove().expect("prove failed");

    let mut tampered = inputs.public_input_hash;
    if let Some(last) = tampered.last_mut() {
        *last ^= 0x01;
    }
    assert!(
        !verify_with_generated_vk(
            &vk,
            &proof.proof_a,
            &proof.proof_b,
            &proof.proof_c,
            tampered
        ),
        "verification must fail for a tampered public input"
    );
}

#[test]
fn create_dvp_rejects_zero_amount() {
    assert_terms_unprovable("zero delivery amount", |terms| terms.delivery_amount = 0);
    assert_terms_unprovable("zero payment amount", |terms| terms.payment_amount = 0);
}

#[test]
fn create_dvp_rejects_equal_assets() {
    assert_terms_unprovable("delivery and payment mints are equal", |terms| {
        terms.payment_mint = terms.delivery_mint
    });
}

#[test]
fn create_dvp_rejects_earliest_after_expiry() {
    assert_terms_unprovable("earliest settlement after expiry", |terms| {
        terms.earliest_settlement_timestamp = Some(terms.expiry_timestamp + 1)
    });
}

#[test]
fn create_dvp_rejects_wrong_owner_hash() {
    assert_unprovable(
        "the DvP UTXO is not owned by the DvP authority",
        DVP_ADDRESS_TREE_ID,
        false,
        |inputs| inputs.dvp_utxo.owner_hash = fe(9),
    );
}

#[test]
fn create_dvp_rejects_wrong_data_hash() {
    assert_unprovable(
        "the DvP UTXO data hash is not dvp_id",
        DVP_ADDRESS_TREE_ID,
        false,
        |inputs| inputs.dvp_utxo.data_hash = fe(9),
    );
}

#[test]
fn create_dvp_rejects_wrong_address_tree() {
    // Every hash and blinding is consistent with an address in tree 1; only
    // the D6 pin to DVP_ADDRESS_TREE_ID refuses it.
    assert_unprovable(
        "the address slot is not in DVP_ADDRESS_TREE_ID",
        DVP_ADDRESS_TREE_ID + 1,
        true,
        |_| {},
    );
}

#[test]
fn create_dvp_rejects_wrong_creator() {
    assert_unprovable(
        "the public creator is not the terms creator",
        DVP_ADDRESS_TREE_ID,
        false,
        |inputs| inputs.creator_id = fe(9),
    );
}

#[test]
fn create_dvp_rejects_non_derived_blinding() {
    assert_unprovable(
        "the DvP UTXO blinding is not derived from the create seed",
        DVP_ADDRESS_TREE_ID,
        false,
        |inputs| inputs.dvp_utxo.blinding = fe(9),
    );
}
