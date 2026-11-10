use groth16_solana::{
    decompression::{decompress_g1, decompress_g2},
    groth16::Groth16Verifier,
    vk::gnark::{parse_gnark_vk_bytes, Groth16VerifyingkeyOwned},
};
use solana_address::Address;
use spr_client::ProofInputUtxo;
use spr_hasher::primitives::{hash_bytes, solana_owner_identity};
use spr_keypair::{hash::poseidon, ViewingKey};
use spr_program::program::PdaOwner;
use spr_transaction::{instructions::transact::PrivateTxHash, utxo::Blinding};
use swap_program::{
    instructions::{
        make::{verify_make_proof, MakeProof, MakePublicInput},
        verifier::{verify_groth16, CompressedGroth16Proof},
    },
    verifying_keys::make::VERIFYINGKEY,
};
use swap_prover::{CircuitId, MakeProofInputs, OrderTermsProofInput, PROVER};
use swap_sdk::{order_authority_pda, state::DataHash};

fn build_dir() -> std::path::PathBuf {
    PROVER.keys_dir(CircuitId::Make)
}

fn ensure_keys() {
    let dir = build_dir();
    if !dir.join("pk.bin").exists() || !dir.join("vk.bin").exists() {
        PROVER
            .setup_insecure_test_keys(CircuitId::Make, &dir)
            .expect("setup failed");
    }
}

fn generated_vk() -> Groth16VerifyingkeyOwned {
    let bytes = std::fs::read(build_dir().join("vk.bin")).expect("read vk.bin");
    parse_gnark_vk_bytes(&bytes).expect("parse vk.bin")
}

fn fe(byte: u8) -> [u8; 32] {
    let mut out = [0u8; 32];
    out[31] = byte;
    out
}

fn blinding(byte: u8) -> Blinding {
    let mut out = [0u8; 32];
    out[31] = byte;
    out
}

/// The maker's nullifier pubkey; with the maker's owner identity it forms the
/// order terms' maker owner hash.
const MAKER_NULLIFIER_PK: u8 = 99;

/// `solana_owner_identity(maker)`, which the program computes from the maker
/// signer.
fn owner_pk_field(maker: &Address) -> [u8; 32] {
    solana_owner_identity(maker.as_array()).expect("owner identity")
}

fn maker_owner_hash(maker: &Address) -> [u8; 32] {
    poseidon(&[&owner_pk_field(maker), &fe(MAKER_NULLIFIER_PK)]).expect("maker owner hash")
}

fn sample_order(maker: &Address) -> OrderTermsProofInput {
    let maker_viewing_pk = *ViewingKey::new().pubkey().as_bytes();
    OrderTermsProofInput {
        destination_asset: hash_bytes(&[2u8; 32]).expect("destination asset"),
        destination_amount: 250,
        maker_owner_hash: maker_owner_hash(maker),
        maker_viewing_pk,
        expiry: 1_700_000_000,
        taker_pk_fe: fe(123),
    }
}

/// Both outputs of a make land in the same tree. A non-zero id keeps the test
/// honest: a dropped tree id would change every commitment.
const OUTPUT_TREE_ID: u16 = 3;
/// The source is spent from another tree, so a source hashed under the output
/// tree's id would not match.
const SOURCE_TREE_ID: u16 = 2;
/// The amount locked in the order UTXO; the source funds it and the change.
const ORDER_AMOUNT: u64 = 1_000;

const MAKER: Address = Address::new_from_array([42u8; 32]);
const OTHER_MAKER: Address = Address::new_from_array([43u8; 32]);

/// The owner hash the program derives on chain for `maker`'s order-authority
/// PDA (`OrderAuthority::owner_hash`).
fn order_owner_hash(maker: &Address) -> [u8; 32] {
    *PdaOwner::new(&order_authority_pda(maker))
        .expect("pda owner")
        .owner_hash()
}

/// A make witness. The honest case has every field derived from `MAKER`; the
/// failing tests change one of them.
struct MakeCase {
    destination_amount: u64,
    change_amount: u64,
    /// The owner the order UTXO actually commits to.
    order_utxo_owner_hash: [u8; 32],
    /// The order-authority owner hash public input.
    order_owner_hash: [u8; 32],
    /// The maker the order terms name.
    terms_maker: Address,
    /// The maker whose identity is the `maker_owner_pk_field` public input.
    signer: Address,
    /// The owner the spent source commits to.
    source_owner_hash: [u8; 32],
    /// The source's data hash; a plain token UTXO has none.
    source_data_hash: [u8; 32],
    /// The source's asset.
    source_mint: Address,
}

const SOURCE_MINT: Address = Address::new_from_array([1u8; 32]);

impl Default for MakeCase {
    fn default() -> Self {
        let owner_hash = order_owner_hash(&MAKER);
        Self {
            destination_amount: 250,
            change_amount: 750,
            order_utxo_owner_hash: owner_hash,
            order_owner_hash: owner_hash,
            terms_maker: MAKER,
            signer: MAKER,
            source_owner_hash: maker_owner_hash(&MAKER),
            source_data_hash: [0u8; 32],
            source_mint: SOURCE_MINT,
        }
    }
}

fn build_inputs(destination_amount: u64, change_amount: u64) -> MakeProofInputs {
    build_case(MakeCase {
        destination_amount,
        change_amount,
        ..MakeCase::default()
    })
}

fn build_case(case: MakeCase) -> MakeProofInputs {
    let MakeCase {
        destination_amount,
        change_amount,
        order_utxo_owner_hash,
        order_owner_hash,
        terms_maker,
        signer,
        source_owner_hash,
        source_data_hash,
        source_mint,
    } = case;
    let maker_owner_pk_field = owner_pk_field(&signer);
    let mut order = sample_order(&terms_maker);
    order.destination_amount = destination_amount;
    let order_utxo = ProofInputUtxo::new(
        order_utxo_owner_hash,
        &SOURCE_MINT,
        ORDER_AMOUNT,
        &blinding(7),
        OUTPUT_TREE_ID,
    )
    .expect("order utxo")
    .with_data_hash(order.data_hash().expect("order data hash"));
    let change = ProofInputUtxo::new(
        order.maker_owner_hash,
        &SOURCE_MINT,
        change_amount,
        &blinding(6),
        OUTPUT_TREE_ID,
    )
    .expect("change utxo");
    let source = ProofInputUtxo::new(
        source_owner_hash,
        &source_mint,
        ORDER_AMOUNT + change_amount,
        &blinding(5),
        SOURCE_TREE_ID,
    )
    .expect("source utxo")
    .with_data_hash(source_data_hash);
    let source_input_hash = source.hash().expect("source hash");
    let private_tx_blinding = fe(21);
    let private_tx_hash = PrivateTxHash::new(
        &[source_input_hash, [0u8; 32]],
        &[
            change.hash().expect("change hash"),
            order_utxo.hash().expect("order utxo hash"),
        ],
        &private_tx_blinding,
    )
    .hash()
    .expect("private tx hash");
    let public_input_hash = MakePublicInput {
        private_tx_hash: &private_tx_hash,
        order_owner_hash: &order_owner_hash,
        maker_owner_pk_field: &maker_owner_pk_field,
    }
    .hash()
    .expect("public input hash");
    MakeProofInputs {
        public_input_hash,
        private_tx_hash,
        order_owner_hash,
        maker_owner_pk_field,
        maker_nullifier_pk: fe(MAKER_NULLIFIER_PK),
        order,
        source,
        order_utxo,
        change,
        private_tx_blinding,
    }
}

fn sample_inputs() -> MakeProofInputs {
    build_inputs(250, 750)
}

/// Verifies `proof` as `process_make_ix` does for the maker signer `signer`
/// and the order-authority owner hash `order_owner_hash`.
fn verify_as_program(
    proof: &MakeProof,
    private_tx_hash: &[u8; 32],
    order_owner_hash: &[u8; 32],
    signer: &Address,
) -> Result<(), impl std::fmt::Debug> {
    verify_make_proof(
        proof,
        MakePublicInput {
            private_tx_hash,
            order_owner_hash,
            maker_owner_pk_field: &owner_pk_field(signer),
        },
    )
}

/// Order UTXO owners other than the maker's order-authority PDA: the maker
/// itself, or another maker's PDA.
fn foreign_order_owners() -> [(&'static str, [u8; 32]); 2] {
    [
        ("maker-owned", maker_owner_hash(&MAKER)),
        ("other maker's PDA", order_owner_hash(&OTHER_MAKER)),
    ]
}

fn verify_with_generated_vk(
    vk: &Groth16VerifyingkeyOwned,
    proof_a: &[u8; 32],
    proof_b: &[u8; 64],
    proof_c: &[u8; 32],
    public_input: [u8; 32],
) -> bool {
    let a = match decompress_g1(proof_a) {
        Ok(g1) => g1,
        Err(_) => return false,
    };
    let b = match decompress_g2(proof_b) {
        Ok(g2) => g2,
        Err(_) => return false,
    };
    let c = match decompress_g1(proof_c) {
        Ok(g1) => g1,
        Err(_) => return false,
    };
    let public_inputs = [public_input];
    let borrowed = vk.as_borrowed();
    let mut verifier = match Groth16Verifier::new(&a, &b, &c, &public_inputs, &borrowed) {
        Ok(parsed) => parsed,
        Err(_) => return false,
    };
    verifier.verify().is_ok()
}

#[test]
fn program_vk_has_no_commitment() {
    assert_eq!(VERIFYINGKEY.nr_pubinputs, 1);
    assert!(
        VERIFYINGKEY.vk_commitment.is_none(),
        "make circuit is standard Groth16: no BSB22 commitment"
    );
    assert_eq!(
        VERIFYINGKEY.vk_ic.len(),
        2,
        "standard Groth16 vk_ic length must be public_inputs + 1"
    );
}

#[test]
fn make_prove_verify() {
    ensure_keys();
    let vk = generated_vk();

    let inputs = sample_inputs();
    let proof = inputs.prove().expect("prove failed");

    let proof_a_zero = proof.proof_a.iter().all(|byte| *byte == 0);
    assert!(!proof_a_zero, "proof_a must not be all zero");

    assert!(
        verify_with_generated_vk(
            &vk,
            &proof.proof_a,
            &proof.proof_b,
            &proof.proof_c,
            inputs.public_input_hash,
        ),
        "groth16 proof must verify against the make verifying key with the public input hash as the sole public input"
    );

    let proof: MakeProof = proof.into();
    verify_groth16(
        CompressedGroth16Proof {
            a: &proof.proof_a,
            b: &proof.proof_b,
            c: &proof.proof_c,
        },
        inputs.public_input_hash,
        &VERIFYINGKEY,
    )
    .expect("the committed make VERIFYINGKEY must accept the proof; run `just ensure-swap-keys`");
}

#[test]
fn make_rejects_tampered_public_input() {
    ensure_keys();
    let vk = generated_vk();

    let inputs = sample_inputs();
    let proof = inputs.prove().expect("prove failed");

    let mut tampered = inputs.public_input_hash;
    tampered[31] ^= 0x01;

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
fn make_rejects_tampered_order_term() {
    ensure_keys();

    let inputs = build_inputs(0, 750);

    assert!(
        inputs.prove().is_err(),
        "proving must fail when destination_amount is zero (constraint violation)"
    );
}

#[test]
fn make_zero_change_proves() {
    ensure_keys();
    let vk = generated_vk();

    let inputs = build_inputs(250, 0);
    let proof = inputs.prove().expect("prove failed");

    assert!(
        verify_with_generated_vk(
            &vk,
            &proof.proof_a,
            &proof.proof_b,
            &proof.proof_c,
            inputs.public_input_hash,
        ),
        "a zero-value change output is non-dummy: its real utxo hash enters private_tx_hash and the proof must verify, matching SPP"
    );
}

/// A maker who owns the order UTXO, or puts it under another maker's PDA,
/// could spend it outside its own order authority. With the maker's PDA owner
/// hash as the public input, no witness proves either.
#[test]
fn make_rejects_order_not_owned_by_makers_pda() {
    ensure_keys();

    for (name, order_utxo_owner_hash) in foreign_order_owners() {
        let inputs = build_case(MakeCase {
            order_utxo_owner_hash,
            ..MakeCase::default()
        });
        assert!(
            inputs.prove().is_err(),
            "{name}: proving must fail when the order utxo owner is not the maker's order-authority PDA"
        );
    }
}

/// A prover that binds the order UTXO to an owner hash of its choice gets a
/// valid proof for that public input, but the program hashes in the owner
/// hash of the signing maker's PDA, so the proof does not verify on chain.
#[test]
fn make_program_rejects_proof_for_other_owner_hash() {
    ensure_keys();

    for (name, owner_hash) in foreign_order_owners() {
        let inputs = build_case(MakeCase {
            order_utxo_owner_hash: owner_hash,
            order_owner_hash: owner_hash,
            ..MakeCase::default()
        });
        let proof: MakeProof = inputs.prove().expect("prove failed").into();

        verify_as_program(&proof, &inputs.private_tx_hash, &owner_hash, &MAKER).unwrap_or_else(
            |e| panic!("{name}: the proof verifies against its own owner hash: {e:?}"),
        );
        assert!(
            verify_as_program(
                &proof,
                &inputs.private_tx_hash,
                &order_owner_hash(&MAKER),
                &MAKER
            )
            .is_err(),
            "{name}: the signing maker's PDA owner hash must reject the proof"
        );
    }
}

/// Terms naming maker M2 in a make signed by M1 would create an order owned
/// by PDA(M1) that only M2 may cancel, and M2's cancel derives PDA(M2): the
/// order could never be cancelled. The terms' maker must be the signer.
#[test]
fn make_rejects_terms_maker_other_than_signer() {
    ensure_keys();

    // The source belongs to the terms' maker, so only the signer check fails.
    let inputs = build_case(MakeCase {
        terms_maker: OTHER_MAKER,
        source_owner_hash: maker_owner_hash(&OTHER_MAKER),
        ..MakeCase::default()
    });
    assert!(
        inputs.prove().is_err(),
        "proving must fail when the terms' maker is not the signer"
    );

    // A proof whose public maker identity is the terms' maker M2 verifies only
    // against M2's identity; the program hashes in the signer M1's.
    let inputs = build_case(MakeCase {
        terms_maker: OTHER_MAKER,
        signer: OTHER_MAKER,
        source_owner_hash: maker_owner_hash(&OTHER_MAKER),
        ..MakeCase::default()
    });
    let proof: MakeProof = inputs.prove().expect("prove failed").into();
    let order_owner_hash = order_owner_hash(&MAKER);
    verify_as_program(
        &proof,
        &inputs.private_tx_hash,
        &order_owner_hash,
        &OTHER_MAKER,
    )
    .expect("the proof verifies against the terms' maker identity");
    assert!(
        verify_as_program(&proof, &inputs.private_tx_hash, &order_owner_hash, &MAKER).is_err(),
        "the signer's identity must reject a proof for another maker's terms"
    );
}

/// The maker's PDA owns every open order, and the maker knows each order's
/// opening. A make that spends one, or any other UTXO the maker does not own,
/// would let the maker pull an order before expiry. The source must be the
/// maker signer's own.
#[test]
fn make_rejects_source_not_owned_by_maker() {
    ensure_keys();

    let open_order_data_hash = sample_order(&MAKER).data_hash().expect("order data hash");
    for (name, source_owner_hash, source_data_hash) in [
        (
            "maker's open order",
            order_owner_hash(&MAKER),
            open_order_data_hash,
        ),
        ("maker's PDA", order_owner_hash(&MAKER), [0u8; 32]),
        ("another maker", maker_owner_hash(&OTHER_MAKER), [0u8; 32]),
    ] {
        let inputs = build_case(MakeCase {
            source_owner_hash,
            source_data_hash,
            ..MakeCase::default()
        });
        assert!(
            inputs.prove().is_err(),
            "{name}: proving must fail when the source is not owned by the maker"
        );
    }
}

/// The source is a plain token UTXO of the order's asset: a data-bearing
/// UTXO or one of another asset does not fund the order.
#[test]
fn make_rejects_source_with_data_or_other_asset() {
    ensure_keys();

    let with_data = build_case(MakeCase {
        source_data_hash: fe(77),
        ..MakeCase::default()
    });
    assert!(
        with_data.prove().is_err(),
        "proving must fail when the source carries data"
    );
    let other_asset = build_case(MakeCase {
        source_mint: Address::new_from_array([9u8; 32]),
        ..MakeCase::default()
    });
    assert!(
        other_asset.prove().is_err(),
        "proving must fail when the source holds another asset"
    );
}
