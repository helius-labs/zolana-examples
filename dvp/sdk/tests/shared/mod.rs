use dvp_program::{
    instructions::{create_dvp::CreateDvpPublicInput, shared::u64_right_align},
    DVP_ADDRESS_TREE_ID,
};
use dvp_prover::{CreateDvpProofInputs, ProofInputUtxo, SwapDvpProofInput};
use dvp_sdk::state::SwapDvp;
use solana_address::Address;
use zolana_hasher::primitives::solana_owner_identity;
use zolana_keypair::{
    constants::BLINDING_LEN, hash::poseidon, NullifierKey, PublicKey, ShieldedAddress, ViewingKey,
};
use zolana_transaction::{
    instructions::transact::PrivateTxHash,
    utxo::{
        derive_output_blinding_seed, derive_private_tx_blinding, derive_transact_output_blinding,
    },
    SOL_MINT,
};

/// The tree the DvP UTXO is appended to. A non-zero id keeps the tests
/// honest: a dropped tree id would change the commitment.
pub const OUTPUT_TREE_ID: u16 = 7;

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
fn pda_owner_hash(pda: &Address) -> [u8; 32] {
    let identity = solana_owner_identity(pda.as_array()).expect("pda identity");
    let nullifier_pk = NullifierKey::from_secret([0u8; BLINDING_LEN])
        .pubkey()
        .expect("zero-secret nullifier pubkey");
    poseidon(&[&identity, &nullifier_pk]).expect("pda owner hash")
}

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
pub fn derive_create_inputs(terms: &SwapDvp) -> CreateDvpProofInputs {
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
pub fn reseal(
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
