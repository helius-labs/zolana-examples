use groth16_solana::{
    decompression::{decompress_g1, decompress_g2},
    groth16::Groth16Verifier,
    vk::gnark::{parse_gnark_vk_bytes, Groth16VerifyingkeyOwned},
};
use solana_address::Address;
use solana_pubkey::Pubkey;
use spr_client::ProofInputUtxo;
use spr_hasher::primitives::solana_owner_identity;
use spr_keypair::hash::poseidon;
use spr_transaction::{instructions::transact::PrivateTxHash, utxo::Blinding};
use timelock_escrow_program::{
    error::TimelockEscrowError,
    instructions::{
        escrow::{verify_escrow_proof, EscrowProof, EscrowPublicInput},
        shared::EscrowAuthority,
        verifier::{verify_groth16, CompressedGroth16Proof},
    },
    verifying_keys::escrow::VERIFYINGKEY,
};
use timelock_escrow_prover::{CircuitId, EscrowProofInputs, EscrowTermsProofInput, PROVER};
use timelock_escrow_sdk::{escrow_authority_pda, escrow_authority_pda_and_bump, state::DataHash};

mod shared;
use shared::escrow_utxo_owner_hash;

fn build_dir() -> std::path::PathBuf {
    PROVER.keys_dir(CircuitId::Escrow)
}

fn ensure_keys() {
    let dir = build_dir();
    if !dir.join("pk.bin").exists() || !dir.join("vk.bin").exists() {
        PROVER
            .setup_insecure_test_keys(CircuitId::Escrow, &dir)
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

/// The creator signer: its owner identity is a public input, and its
/// escrow-authority PDA owns the escrow utxo.
fn creator() -> Pubkey {
    Pubkey::new_from_array([42u8; 32])
}

fn owner_pk_field(creator: &Pubkey) -> [u8; 32] {
    solana_owner_identity(creator.as_array()).expect("creator owner identity")
}

fn creator_nullifier_pk() -> [u8; 32] {
    fe(77)
}

/// The owner hash the program passes for `creator`: its escrow-authority PDA
/// with the zero-secret nullifier key.
fn pda_owner_hash(creator: &Pubkey) -> [u8; 32] {
    escrow_utxo_owner_hash(escrow_authority_pda(creator).as_array())
}

fn sample_terms() -> EscrowTermsProofInput {
    EscrowTermsProofInput {
        owner_hash: poseidon(&[&owner_pk_field(&creator()), &creator_nullifier_pk()])
            .expect("creator owner hash"),
        unlock: 1_700_000_000,
    }
}

/// Both outputs of an escrow land in the same tree. A non-zero id keeps the test
/// honest: a dropped tree id would change every commitment.
const OUTPUT_TREE_ID: u16 = 3;
/// The tree the source is spent from, distinct from the output tree so a
/// swapped tree id changes the source commitment.
const INPUT_TREE_ID: u16 = 2;

/// The program's public input for `creator`: what `process_escrow_ix`
/// derives from the creator signer.
fn program_public_input(private_tx_hash: &[u8; 32], creator: &Pubkey) -> [u8; 32] {
    EscrowPublicInput {
        private_tx_hash,
        escrow_owner_hash: &pda_owner_hash(creator),
        owner_pk_field: &owner_pk_field(creator),
    }
    .hash()
    .expect("escrow public input hash")
}

/// A plain token utxo of `amount` in the escrow asset, spent from the input
/// tree.
fn token_utxo(owner_hash: [u8; 32], amount: u64) -> ProofInputUtxo {
    ProofInputUtxo::new(
        owner_hash,
        &SOURCE_MINT,
        amount,
        &blinding(5),
        INPUT_TREE_ID,
    )
    .expect("token utxo")
}

const SOURCE_MINT: Address = Address::new_from_array([1u8; 32]);

/// Escrow proof inputs spending `source`, whose escrow utxo is owned by
/// `escrow_owner_hash`, self-consistent: the witness's public escrow owner
/// equals the utxo owner and the private tx hash covers every utxo.
fn build_inputs_with(
    source: ProofInputUtxo,
    escrow_owner_hash: [u8; 32],
    escrow_amount: u64,
    change_amount: u64,
) -> EscrowProofInputs {
    let terms = sample_terms();
    let escrow_utxo = ProofInputUtxo::new(
        escrow_owner_hash,
        &SOURCE_MINT,
        escrow_amount,
        &blinding(7),
        OUTPUT_TREE_ID,
    )
    .expect("escrow utxo")
    .with_data_hash(terms.data_hash().expect("terms data hash"));
    let change = ProofInputUtxo::new(
        terms.owner_hash,
        &SOURCE_MINT,
        change_amount,
        &blinding(6),
        OUTPUT_TREE_ID,
    )
    .expect("change utxo");
    let private_tx_blinding = fe(21);
    let private_tx_hash = PrivateTxHash::new(
        &[source.hash().expect("source hash"), [0u8; 32]],
        &[
            change.hash().expect("change hash"),
            escrow_utxo.hash().expect("escrow utxo hash"),
        ],
        &private_tx_blinding,
    )
    .hash()
    .expect("private tx hash");
    let owner_pk_field = owner_pk_field(&creator());
    let public_input_hash = EscrowPublicInput {
        private_tx_hash: &private_tx_hash,
        escrow_owner_hash: &escrow_owner_hash,
        owner_pk_field: &owner_pk_field,
    }
    .hash()
    .expect("escrow public input hash");
    EscrowProofInputs {
        public_input_hash,
        private_tx_hash,
        escrow_owner_hash,
        owner_pk_field,
        nullifier_pk: creator_nullifier_pk(),
        terms,
        source,
        escrow_utxo,
        change,
        private_tx_blinding,
    }
}

/// Escrow proof inputs spending the creator's own source of
/// `escrow_amount + change_amount`, whose escrow utxo is owned by
/// `escrow_owner_hash`.
fn build_inputs_owned_by(
    escrow_owner_hash: [u8; 32],
    escrow_amount: u64,
    change_amount: u64,
) -> EscrowProofInputs {
    let source = token_utxo(sample_terms().owner_hash, escrow_amount + change_amount);
    build_inputs_with(source, escrow_owner_hash, escrow_amount, change_amount)
}

fn build_inputs(escrow_amount: u64, change_amount: u64) -> EscrowProofInputs {
    build_inputs_owned_by(pda_owner_hash(&creator()), escrow_amount, change_amount)
}

fn sample_inputs() -> EscrowProofInputs {
    build_inputs(250, 750)
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
        "escrow circuit is standard Groth16: no BSB22 commitment"
    );
    assert_eq!(
        VERIFYINGKEY.vk_ic.len(),
        2,
        "standard Groth16 vk_ic length must be public_inputs + 1"
    );
}

#[test]
fn escrow_prove_verify() {
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
            program_public_input(&inputs.private_tx_hash, &creator()),
        ),
        "groth16 proof must verify against the escrow verifying key with the program's public-input hash"
    );

    let proof: EscrowProof = proof.into();
    verify_groth16(
        CompressedGroth16Proof {
            a: &proof.proof_a,
            b: &proof.proof_b,
            c: &proof.proof_c,
            commitment: None,
        },
        program_public_input(&inputs.private_tx_hash, &creator()),
        &VERIFYINGKEY,
    )
    .expect(
        "the committed escrow VERIFYINGKEY must accept the proof; run `just ensure-escrow-keys`",
    );
}

#[test]
fn escrow_rejects_tampered_public_input() {
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
fn escrow_rejects_zero_amount() {
    ensure_keys();

    let inputs = build_inputs(0, 750);

    assert!(
        inputs.prove().is_err(),
        "proving must fail when the escrow amount is zero (constraint violation)"
    );
}

#[test]
fn escrow_zero_change_proves() {
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

/// The attack the owner binding closes: the creator builds its own escrow
/// transaction with the escrow utxo owned by itself (an owner signer SPP
/// accepts for a data-bearing output) or by another creator's
/// escrow-authority PDA, so it could spend the escrow before unlock without
/// the program. The witness is self-consistent and proves, but the program
/// supplies the creator's PDA owner hash, so it does not verify.
#[test]
fn escrow_rejects_escrow_utxo_not_owned_by_creators_pda() {
    ensure_keys();

    let cases = [
        ("the creator itself", sample_terms().owner_hash),
        (
            "another creator's PDA",
            pda_owner_hash(&Pubkey::new_from_array([43u8; 32])),
        ),
    ];
    for (name, escrow_owner_hash) in cases {
        let inputs = build_inputs_owned_by(escrow_owner_hash, 250, 750);
        let proof = inputs.prove().expect("self-consistent witness proves");

        let proof: EscrowProof = proof.into();
        // What `process_escrow_ix` checks for this creator signer.
        let result = verify_escrow_proof(
            &proof,
            &inputs.private_tx_hash,
            &pda_owner_hash(&creator()),
            &owner_pk_field(&creator()),
        );
        assert_eq!(
            result,
            Err(TimelockEscrowError::ProofVerificationFailed.into()),
            "an escrow utxo owned by {name} must not verify against the creator's public input"
        );
    }
}

/// The circuit binds the escrow utxo owner to the public escrow owner hash:
/// a witness claiming the PDA owner while the utxo is creator-owned fails to
/// prove.
#[test]
fn escrow_rejects_utxo_owner_mismatching_public_owner() {
    ensure_keys();

    let mut inputs = build_inputs_owned_by(sample_terms().owner_hash, 250, 750);
    inputs.escrow_owner_hash = pda_owner_hash(&creator());
    inputs.public_input_hash = program_public_input(&inputs.private_tx_hash, &creator());

    assert!(
        inputs.prove().is_err(),
        "proving must fail when the escrow utxo owner differs from the public escrow owner"
    );
}

/// The terms' owner hash must be built from the creator signer's identity.
#[test]
fn escrow_rejects_terms_of_another_creator() {
    ensure_keys();

    let mut inputs = sample_inputs();
    let other_creator = Pubkey::new_from_array([43u8; 32]);
    inputs.owner_pk_field = owner_pk_field(&other_creator);
    inputs.public_input_hash = EscrowPublicInput {
        private_tx_hash: &inputs.private_tx_hash,
        escrow_owner_hash: &inputs.escrow_owner_hash,
        owner_pk_field: &inputs.owner_pk_field,
    }
    .hash()
    .expect("escrow public input hash");

    assert!(
        inputs.prove().is_err(),
        "proving must fail when the terms' owner hash is not built from the creator signer"
    );
}

/// The timelock bypass the source binding closes: the program signs for the
/// creator's escrow-authority PDA, so a source owned by it would need no
/// creator signature. The creator knows the preimage of its own escrow utxo
/// (nullifier secret 0) and could re-lock 1 lamport of it and take the rest
/// as change before unlock. Every PDA-owned source fails to prove, as does a
/// source of another owner.
#[test]
fn escrow_rejects_source_not_owned_by_creator() {
    ensure_keys();

    let terms = sample_terms();
    let pda = pda_owner_hash(&creator());
    let own_escrow = token_utxo(pda, 1_000).with_data_hash(terms.data_hash().expect("data hash"));
    let cases = [
        ("the creator's own escrow utxo", own_escrow),
        ("a plain utxo of the creator's PDA", token_utxo(pda, 1_000)),
        (
            "another owner",
            token_utxo(
                poseidon(&[
                    &owner_pk_field(&Pubkey::new_from_array([43u8; 32])),
                    &creator_nullifier_pk(),
                ])
                .expect("other owner hash"),
                1_000,
            ),
        ),
    ];
    for (name, source) in cases {
        let inputs = build_inputs_with(source, pda, 1, 999);
        assert!(
            inputs.prove().is_err(),
            "proving must fail when the source is owned by {name}"
        );
    }
}

/// The source must hold the escrow's asset.
#[test]
fn escrow_rejects_source_of_another_asset() {
    ensure_keys();

    let source = ProofInputUtxo::new(
        sample_terms().owner_hash,
        &Address::new_from_array([2u8; 32]),
        1_000,
        &blinding(5),
        INPUT_TREE_ID,
    )
    .expect("source utxo");
    let inputs = build_inputs_with(source, pda_owner_hash(&creator()), 250, 750);
    assert!(
        inputs.prove().is_err(),
        "proving must fail when the source asset differs from the escrow asset"
    );
}

/// The escrow instruction derives the creator's PDA from the
/// `authority_bump` it carries. A non-canonical bump that still derives a PDA
/// names another PDA of the creator, whose owner hash is a different public
/// input, so a proof for the canonical PDA's escrow utxo does not verify.
#[test]
fn escrow_proof_rejects_a_non_canonical_authority_bump() {
    ensure_keys();

    let inputs = sample_inputs();
    let proof: EscrowProof = inputs.prove().expect("prove failed").into();
    let (_, canonical_bump) = escrow_authority_pda_and_bump(&creator());
    let verify_with_bump = |bump: u8| {
        let escrow_authority = EscrowAuthority::new(&creator(), bump)?;
        verify_escrow_proof(
            &proof,
            &inputs.private_tx_hash,
            &escrow_authority.owner_hash()?,
            &owner_pk_field(&creator()),
        )
    };

    assert_eq!(verify_with_bump(canonical_bump), Ok(()));
    let non_canonical_bump = (0..canonical_bump)
        .rev()
        .find(|bump| EscrowAuthority::new(&creator(), *bump).is_ok())
        .expect("a valid non-canonical bump below the canonical one");
    assert_eq!(
        verify_with_bump(non_canonical_bump),
        Err(TimelockEscrowError::ProofVerificationFailed.into())
    );
}
