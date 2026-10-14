use dvp_program::{
    instructions::{reclaim_dvp::ReclaimDvpPublicInput, shared::u64_right_align},
    verifying_keys::reclaim_dvp::VERIFYINGKEY,
};
use dvp_prover::{CircuitId, ProofInputUtxo, ReclaimDvpProofInputs, SwapDvpProofInput};
use dvp_sdk::{
    instructions::reclaim_dvp::ReclaimDvpProofInputParams,
    state::{Deposit, Leg, SwapDvp},
};
use solana_address::Address;
use zolana_hasher::primitives::{hash_bytes, solana_owner_identity};
use zolana_keypair::ShieldedAddress;
use zolana_transaction::{
    instructions::transact::{PrivateTxHash, SppProofOutputUtxo},
    Mint,
};

mod shared;
use shared::{
    assert_single_input_vk, assert_unsatisfied, assert_verifies, ensure_keys, fe, generated_vk,
    pda_owner_hash, sample_terms, verify_with_generated_vk, OUTPUT_TREE_ID,
};

const CIRCUIT: CircuitId = CircuitId::ReclaimDvp;

/// The tree the deposit is spent from. It differs from the output tree, so a
/// swapped tree id changes a commitment the proof binds.
const INPUT_TREE_ID: u16 = 3;
/// Pool asset id of the sample delivery mint; only the mint enters the proof.
const DELIVERY_ASSET_ID: u64 = 2;
/// What a funder sent above the leg amount. The refund returns it too.
const SURPLUS: u64 = 17;

/// The party that funds `leg`.
fn funder(terms: &SwapDvp, leg: Leg) -> Address {
    match leg {
        Leg::Delivery => terms.seller,
        Leg::Payment => terms.buyer,
    }
}

/// Where the refund goes. The circuit leaves the owner free; the tests use
/// the funder's refund destination.
fn refund_owner(terms: &SwapDvp, leg: Leg) -> ShieldedAddress {
    match leg {
        Leg::Delivery => terms.seller_refund_destination,
        Leg::Payment => terms.buyer_refund_destination,
    }
}

/// A deposit on `leg` holding the leg amount plus [`SURPLUS`].
fn deposit(terms: &SwapDvp, leg: Leg) -> Deposit {
    Deposit {
        terms: terms.clone(),
        leg,
        mint: match leg {
            Leg::Delivery => Mint::new(terms.delivery_mint, DELIVERY_ASSET_ID),
            Leg::Payment => Mint::SOL,
        },
        amount: leg.amount(terms) + SURPLUS,
        blinding: fe(21),
        tree_id: INPUT_TREE_ID,
        leaf_index: 5,
    }
}

fn params(signer: Address, leg: Leg) -> ReclaimDvpProofInputParams {
    let terms = sample_terms();
    let deposit = deposit(&terms, leg);
    let refund = SppProofOutputUtxo {
        asset: deposit.mint,
        amount: deposit.amount,
        blinding: fe(31),
        owner_address: Some(refund_owner(&terms, leg)),
        ..Default::default()
    };
    ReclaimDvpProofInputParams {
        terms,
        signer,
        deposit,
        refund,
        refund_tree_id: OUTPUT_TREE_ID,
        private_tx_blinding: fe(41),
    }
}

fn sdk_inputs(leg: Leg) -> ReclaimDvpProofInputs {
    params(funder(&sample_terms(), leg), leg)
        .to_proof_inputs()
        .expect("sdk proof inputs")
}

/// The reclaim witness of the sample terms for `signer` taking back the
/// `leg` deposit, derived independently of `ReclaimDvpProofInputParams` and
/// without its checks, so the negative tests can hand the circuit a signer
/// the sdk would refuse.
fn derive_reclaim_inputs(signer: Address, leg: Leg) -> ReclaimDvpProofInputs {
    let terms = sample_terms();
    let (dvp_authority, _) = terms.dvp_authority().expect("dvp authority");
    let owner_hash = pda_owner_hash(&dvp_authority);
    let mint = leg.mint(&terms);
    let amount = leg.amount(&terms) + SURPLUS;
    let refund_owner_hash = refund_owner(&terms, leg)
        .owner_hash()
        .expect("refund owner hash");
    let mut inputs = ReclaimDvpProofInputs {
        public_input_hash: [0u8; 32],
        private_tx_hash: [0u8; 32],
        creator_id: solana_owner_identity(terms.creator.as_array()).expect("creator id"),
        dvp_id: terms.dvp_id().expect("dvp_id"),
        dvp_authority_owner_hash: owner_hash,
        signer_id: solana_owner_identity(signer.as_array()).expect("signer id"),
        terms: SwapDvpProofInput::try_from(&terms).expect("terms proof input"),
        leg: leg == Leg::Payment,
        deposit: ProofInputUtxo::new(owner_hash, &mint, amount, &fe(21), INPUT_TREE_ID)
            .expect("deposit"),
        refund: ProofInputUtxo::new(refund_owner_hash, &mint, amount, &fe(31), OUTPUT_TREE_ID)
            .expect("refund"),
        private_tx_blinding: fe(41),
    };
    reseal(&mut inputs);
    inputs
}

/// Recomputes the private tx hash and the public input hash from the other
/// fields. A negative test tampers one field and reseals, so the proof fails
/// on exactly that field and not on a stale hash.
fn reseal(inputs: &mut ReclaimDvpProofInputs) {
    inputs.private_tx_hash = PrivateTxHash::new(
        &[inputs.deposit.hash().expect("deposit hash")],
        &[inputs.refund.hash().expect("refund hash"), [0u8; 32]],
        &inputs.private_tx_blinding,
    )
    .hash()
    .expect("private tx hash");
    inputs.public_input_hash = ReclaimDvpPublicInput {
        private_tx_hash: &inputs.private_tx_hash,
        creator_id: &inputs.creator_id,
        dvp_id: &inputs.dvp_id,
        dvp_authority_owner_hash: &inputs.dvp_authority_owner_hash,
        signer_id: &inputs.signer_id,
    }
    .hash()
    .expect("public input hash");
}

/// Derives the witness for `signer` and the `leg` deposit, tampers it,
/// reseals it, and asserts the circuit refuses to prove it.
fn assert_unprovable(
    reason: &str,
    signer: Address,
    leg: Leg,
    tamper: impl FnOnce(&mut ReclaimDvpProofInputs),
) {
    ensure_keys(CIRCUIT);
    let mut inputs = derive_reclaim_inputs(signer, leg);
    tamper(&mut inputs);
    reseal(&mut inputs);
    assert_unsatisfied(inputs.prove(), reason);
}

/// Asserts the sdk refuses the tampered parameters.
fn assert_sdk_rejects(
    reason: &str,
    signer: Address,
    leg: Leg,
    tamper: impl FnOnce(&mut ReclaimDvpProofInputParams),
) {
    let mut params = params(signer, leg);
    tamper(&mut params);
    assert!(
        params.to_proof_inputs().is_err(),
        "the sdk must reject: {reason}"
    );
}

fn prove_and_verify(inputs: &ReclaimDvpProofInputs) {
    let proof = inputs.prove().expect("prove failed");
    assert_verifies(CIRCUIT, &proof, inputs.public_input_hash, &VERIFYINGKEY);
}

#[test]
fn program_vk_has_no_commitment() {
    assert_single_input_vk(&VERIFYINGKEY);
}

/// The sdk derivation matches the independent one for both legs, so the
/// negative tests below start from the witness the sdk would build.
#[test]
fn sdk_inputs_match_independent_derivation() {
    let terms = sample_terms();
    for leg in [Leg::Delivery, Leg::Payment] {
        assert_eq!(
            sdk_inputs(leg),
            derive_reclaim_inputs(funder(&terms, leg), leg),
            "{leg:?}"
        );
    }
}

#[test]
fn seller_reclaims_delivery() {
    ensure_keys(CIRCUIT);
    prove_and_verify(&sdk_inputs(Leg::Delivery));
}

#[test]
fn buyer_reclaims_payment() {
    ensure_keys(CIRCUIT);
    prove_and_verify(&sdk_inputs(Leg::Payment));
}

#[test]
fn reclaim_dvp_rejects_tampered_public_input() {
    ensure_keys(CIRCUIT);
    let inputs = sdk_inputs(Leg::Delivery);
    let proof = inputs.prove().expect("prove failed");

    let mut tampered = inputs.public_input_hash;
    if let Some(last) = tampered.last_mut() {
        *last ^= 0x01;
    }
    assert!(
        !verify_with_generated_vk(
            &generated_vk(CIRCUIT),
            &proof.proof_a,
            &proof.proof_b,
            &proof.proof_c,
            tampered
        ),
        "verification must fail for a tampered public input"
    );
}

#[test]
fn reclaim_dvp_rejects_signer_of_neither_party() {
    let outsider = Address::new_from_array([9; 32]);
    assert_sdk_rejects(
        "the signer is neither party",
        outsider,
        Leg::Delivery,
        |_| {},
    );
    for leg in [Leg::Delivery, Leg::Payment] {
        assert_unprovable("the signer is neither party", outsider, leg, |_| {});
    }
}

#[test]
fn reclaim_dvp_rejects_seller_taking_payment() {
    let seller = sample_terms().seller;
    assert_sdk_rejects(
        "the seller does not fund the payment leg",
        seller,
        Leg::Payment,
        |_| {},
    );
    // Leg bit 1: the payment leg is the buyer's.
    assert_unprovable(
        "the payment leg is the buyer's",
        seller,
        Leg::Payment,
        |_| {},
    );
    // Leg bit 0: the seller's leg is delivery, not the payment asset.
    assert_unprovable(
        "the seller's leg is not the payment asset",
        seller,
        Leg::Payment,
        |inputs| inputs.leg = false,
    );
}

#[test]
fn reclaim_dvp_rejects_deposit_with_data_hash() {
    // A UTXO with data, such as the DvP UTXO, is never a deposit.
    assert_unprovable(
        "the deposit carries a data hash",
        sample_terms().seller,
        Leg::Delivery,
        |inputs| inputs.deposit.data_hash = fe(9),
    );
}

#[test]
fn reclaim_dvp_rejects_deposit_with_wrong_owner() {
    assert_unprovable(
        "the deposit is not owned by the DvP authority",
        sample_terms().seller,
        Leg::Delivery,
        |inputs| inputs.deposit.owner_hash = fe(9),
    );
}

#[test]
fn reclaim_dvp_rejects_refund_amount_mismatch() {
    let terms = sample_terms();
    assert_sdk_rejects(
        "the refund does not return the whole deposit",
        terms.seller,
        Leg::Delivery,
        |params| params.refund.amount -= 1,
    );
    let short = Leg::Delivery.amount(&terms) + SURPLUS - 1;
    assert_unprovable(
        "the refund does not return the whole deposit",
        terms.seller,
        Leg::Delivery,
        |inputs| inputs.refund.amount = u64_right_align(short),
    );
}

#[test]
fn reclaim_dvp_rejects_refund_asset_mismatch() {
    let terms = sample_terms();
    assert_sdk_rejects(
        "the refund is not the deposit asset",
        terms.seller,
        Leg::Delivery,
        |params| params.refund.asset = Mint::SOL,
    );
    let payment_asset = hash_bytes(terms.payment_mint.as_array()).expect("payment asset");
    assert_unprovable(
        "the refund is not the deposit asset",
        terms.seller,
        Leg::Delivery,
        |inputs| inputs.refund.asset = payment_asset,
    );
}

#[test]
fn reclaim_dvp_rejects_wrong_creator() {
    assert_unprovable(
        "the public creator is not the terms creator",
        sample_terms().seller,
        Leg::Delivery,
        |inputs| inputs.creator_id = fe(9),
    );
}
