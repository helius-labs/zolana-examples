use dvp_prover::{CircuitId, PROVER};
use dvp_sdk::state::SwapDvp;
use groth16_solana::{
    decompression::{decompress_g1, decompress_g2},
    groth16::Groth16Verifier,
    vk::gnark::{parse_gnark_vk_bytes, Groth16VerifyingkeyOwned},
};
use solana_address::Address;
use zolana_hasher::primitives::solana_owner_identity;
use zolana_keypair::{
    constants::BLINDING_LEN, hash::poseidon, NullifierKey, PublicKey, ShieldedAddress, ViewingKey,
};
use zolana_transaction::SOL_MINT;

/// The tree outputs are appended to. A non-zero id keeps the tests honest: a
/// dropped tree id would change the commitment.
pub const OUTPUT_TREE_ID: u16 = 7;

/// Sets up the circuit's insecure test keys unless they already exist.
pub fn ensure_keys(circuit: CircuitId) {
    let dir = PROVER.keys_dir(circuit);
    if !dir.join("pk.bin").exists() || !dir.join("vk.bin").exists() {
        PROVER
            .setup_insecure_test_keys(circuit, &dir)
            .expect("setup failed");
    }
}

/// The verifying key the setup wrote, as opposed to the committed one in
/// `dvp_program::verifying_keys`.
pub fn generated_vk(circuit: CircuitId) -> Groth16VerifyingkeyOwned {
    let bytes = std::fs::read(PROVER.keys_dir(circuit).join("vk.bin")).expect("read vk.bin");
    parse_gnark_vk_bytes(&bytes).expect("parse vk.bin")
}

pub fn verify_with_generated_vk(
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
    Groth16Verifier::new(&a, &b, &c, &public_inputs, &borrowed)
        .is_ok_and(|mut verifier| verifier.verify().is_ok())
}

pub fn fe(byte: u8) -> [u8; 32] {
    let mut out = [0u8; 32];
    if let Some(last) = out.last_mut() {
        *last = byte;
    }
    out
}

fn destination(seed: u8) -> ShieldedAddress {
    ShieldedAddress {
        signing_pubkey: PublicKey::from_ed25519(&[seed; 32]),
        nullifier_pubkey: NullifierKey::from_secret([seed; 31])
            .pubkey()
            .expect("destination nullifier pubkey"),
        viewing_pubkey: ViewingKey::from_bytes(&[seed; 32])
            .expect("destination viewing key")
            .pubkey(),
    }
}

pub fn sample_terms() -> SwapDvp {
    SwapDvp {
        seller: Address::new_from_array([1; 32]),
        buyer: Address::new_from_array([2; 32]),
        delivery_mint: Address::new_from_array([3; 32]),
        payment_mint: SOL_MINT,
        settlement_authority: Address::new_from_array([4; 32]),
        delivery_amount: 1_000,
        payment_amount: 2_500_000_000,
        expiry_timestamp: 1_800_000_000,
        nonce: 7,
        ref_string: [0; 64],
        seller_settlement_destination: destination(11),
        buyer_settlement_destination: destination(12),
        earliest_settlement_timestamp: Some(1_700_000_000),
        seller_refund_destination: destination(13),
        buyer_refund_destination: destination(14),
        creator: Address::new_from_array([5; 32]),
        blinding: fe(77),
    }
}

/// `Poseidon(solana_owner_identity(pda), Poseidon(0))`, derived without the
/// program's `PdaOwner`.
pub fn pda_owner_hash(pda: &Address) -> [u8; 32] {
    let identity = solana_owner_identity(pda.as_array()).expect("pda identity");
    let nullifier_pk = NullifierKey::from_secret([0u8; BLINDING_LEN])
        .pubkey()
        .expect("zero-secret nullifier pubkey");
    poseidon(&[&identity, &nullifier_pk]).expect("pda owner hash")
}
