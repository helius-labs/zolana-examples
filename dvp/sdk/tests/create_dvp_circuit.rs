use dvp_program::{
    instructions::{
        create_dvp::{CreateDvpProof, CreateDvpPublicInput},
        shared::u64_right_align,
        verifier::{verify_groth16, CompressedGroth16Proof},
    },
    verifying_keys::create_dvp::VERIFYINGKEY,
    DVP_ADDRESS_TREE_ID,
};
use dvp_prover::{CircuitId, CreateDvpProofInputs, ProofInputUtxo, SwapDvpProofInput};
use dvp_sdk::{instructions::create_dvp::CreateDvpProofInputParams, state::SwapDvp};
use zolana_hasher::primitives::solana_owner_identity;
use zolana_keypair::hash::poseidon;
use zolana_transaction::{
    instructions::transact::PrivateTxHash,
    utxo::{
        derive_output_blinding_seed, derive_private_tx_blinding, derive_transact_output_blinding,
    },
    SOL_MINT,
};

mod shared;
use shared::{
    ensure_keys, fe, generated_vk, pda_owner_hash, sample_terms, verify_with_generated_vk,
    OUTPUT_TREE_ID,
};

const CIRCUIT: CircuitId = CircuitId::CreateDvp;

/// The address an address slot owned by `owner_hash` with seed `dvp_id`
/// creates in tree `tree_id`.
fn address(owner_hash: &[u8; 32], dvp_id: &[u8; 32], tree_id: u16) -> [u8; 32] {
    let slot = ProofInputUtxo {
        domain: u64_right_align(2),
        tree_id: u64_right_align(u64::from(tree_id)),
        owner_hash: *owner_hash,
        blinding: *dvp_id,
        ..Default::default()
    };
    poseidon(&[&slot.hash().expect("slot hash"), dvp_id, &[0u8; 32]]).expect("address")
}

/// The create witness of `terms`, derived independently of
/// `CreateDvpProofInputParams` and without validating the terms, so the
/// negative tests can hand the circuit terms the sdk would refuse.
fn derive_create_inputs(terms: &SwapDvp) -> CreateDvpProofInputs {
    let dvp_id = terms.dvp_id().expect("dvp_id");
    let (dvp_authority, _) = terms.dvp_authority().expect("dvp authority");
    let owner_hash = pda_owner_hash(&dvp_authority);
    let dvp_utxo = ProofInputUtxo::new(owner_hash, &SOL_MINT, 0, &[0u8; 32], OUTPUT_TREE_ID)
        .expect("dvp utxo")
        .with_data_hash(dvp_id);
    let mut inputs = CreateDvpProofInputs {
        public_input_hash: [0u8; 32],
        private_tx_hash: [0u8; 32],
        creator_id: solana_owner_identity(terms.creator.as_array()).expect("creator id"),
        dvp_id,
        dvp_authority_owner_hash: owner_hash,
        terms: SwapDvpProofInput::try_from(terms).expect("terms proof input"),
        dvp_utxo,
        private_tx_blinding: [0u8; 32],
    };
    let seed = terms.create_blinding_seed().expect("create seed");
    reseal(&mut inputs, &seed, DVP_ADDRESS_TREE_ID, true);
    inputs
}

/// Recomputes what follows from the other fields: the address (in
/// `address_tree_id`), the private tx hash and the public input hash. With
/// `derive_blindings` the DvP UTXO blinding and the private tx blinding are
/// re-derived from `create_seed` and that address first. A negative test
/// tampers one field and reseals, so the proof fails on exactly that field
/// and not on a stale hash.
fn reseal(
    inputs: &mut CreateDvpProofInputs,
    create_seed: &[u8; 32],
    address_tree_id: u16,
    derive_blindings: bool,
) {
    let address = address(
        &inputs.dvp_authority_owner_hash,
        &inputs.dvp_id,
        address_tree_id,
    );
    if derive_blindings {
        let output_seed =
            derive_output_blinding_seed(&address, create_seed).expect("output blinding seed");
        inputs.dvp_utxo.blinding =
            derive_transact_output_blinding(&address, &output_seed, 0).expect("output blinding");
        inputs.private_tx_blinding =
            derive_private_tx_blinding(&address, create_seed).expect("private tx blinding");
    }
    inputs.private_tx_hash = PrivateTxHash {
        input_hashes: &[[0u8; 32]],
        output_hashes: &[inputs.dvp_utxo.hash().expect("dvp utxo hash"), [0u8; 32]],
        address_nullifiers: Some(&[address]),
        blinding: &inputs.private_tx_blinding,
    }
    .hash()
    .expect("private tx hash");
    inputs.public_input_hash = CreateDvpPublicInput {
        private_tx_hash: &inputs.private_tx_hash,
        creator_id: &inputs.creator_id,
        dvp_id: &inputs.dvp_id,
        dvp_authority_owner_hash: &inputs.dvp_authority_owner_hash,
    }
    .hash()
    .expect("public input hash");
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
    ensure_keys(CIRCUIT);
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
    ensure_keys(CIRCUIT);
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
    ensure_keys(CIRCUIT);
    let vk = generated_vk(CIRCUIT);

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
    ensure_keys(CIRCUIT);
    let mut terms = sample_terms();
    terms.earliest_settlement_timestamp = None;
    let inputs = sdk_inputs(terms);
    let proof = inputs.prove().expect("prove failed");
    assert!(verify_with_generated_vk(
        &generated_vk(CIRCUIT),
        &proof.proof_a,
        &proof.proof_b,
        &proof.proof_c,
        inputs.public_input_hash,
    ));
}

#[test]
fn create_dvp_rejects_tampered_public_input() {
    ensure_keys(CIRCUIT);
    let vk = generated_vk(CIRCUIT);

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
