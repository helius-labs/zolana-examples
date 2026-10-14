use dvp_program::{
    instructions::{settle_dvp::SettleDvpPublicInput, shared::u64_right_align},
    verifying_keys::settle_dvp::VERIFYINGKEY,
    SETTLEMENT_TAG,
};
use dvp_prover::{CircuitId, ProofInputUtxo, SettleDvpProofInputs, SwapDvpProofInput};
use dvp_sdk::{
    instructions::settle_dvp::SettleDvpProofInputParams,
    state::{Deposit, DvpUtxo, Leg, SwapDvp},
};
use solana_address::Address;
use zolana_hasher::primitives::solana_owner_identity;
use zolana_keypair::hash::poseidon;
use zolana_transaction::{
    instructions::transact::PrivateTxHash,
    utxo::{
        derive_output_blinding_seed, derive_private_tx_blinding, derive_transact_output_blinding,
    },
    Mint, SOL_MINT,
};

#[path = "shared/dvp_utxo.rs"]
mod dvp_utxo;
mod shared;
use dvp_utxo::dvp_utxo_blinding;
use shared::{
    assert_single_input_vk, assert_unsatisfied, assert_verifies, ensure_keys, fe, generated_vk,
    pda_owner_hash, sample_terms, verify_with_generated_vk, OUTPUT_TREE_ID,
};

const CIRCUIT: CircuitId = CircuitId::SettleDvp;

/// The tree the DvP UTXO was appended to.
const DVP_UTXO_TREE_ID: u16 = 3;
/// The tree the deposits are spent from. Both input trees differ from the
/// output tree, so a swapped tree id changes a commitment the proof binds.
const DEPOSIT_TREE_ID: u16 = 4;
/// Pool asset id of the sample delivery mint; only the mint enters the proof.
const DELIVERY_ASSET_ID: u64 = 2;
/// Inside the sample terms' settlement window
/// `[earliest 1_700_000_000, expiry 1_800_000_000]`.
const NOW: i64 = 1_750_000_000;
/// What each funder sent above its leg amount: none, or a surplus per leg.
const EXACT: [u64; 2] = [0, 0];
const SURPLUS: [u64; 2] = [17, 23];
/// The BN254 scalar field modulus minus one, which is `-1` in the field.
const MINUS_ONE: [u8; 32] = [
    0x30, 0x64, 0x4e, 0x72, 0xe1, 0x31, 0xa0, 0x29, 0xb8, 0x50, 0x45, 0xb6, 0x81, 0x81, 0x58, 0x5d,
    0x28, 0x33, 0xe8, 0x48, 0x79, 0xb9, 0x70, 0x91, 0x43, 0xe1, 0xf5, 0x93, 0xf0, 0x00, 0x00, 0x00,
];

fn leg_mint(terms: &SwapDvp, leg: Leg) -> Mint {
    match leg {
        Leg::Delivery => Mint::new(terms.delivery_mint, DELIVERY_ASSET_ID),
        Leg::Payment => Mint::SOL,
    }
}

/// A deposit on `leg` holding the leg amount plus `surplus`.
fn deposit(terms: &SwapDvp, leg: Leg, surplus: u64) -> Deposit {
    let (blinding, leaf_index) = match leg {
        Leg::Delivery => (fe(21), 5),
        Leg::Payment => (fe(22), 6),
    };
    Deposit {
        terms: terms.clone(),
        leg,
        mint: leg_mint(terms, leg),
        amount: leg.amount(terms) + surplus,
        blinding,
        tree_id: DEPOSIT_TREE_ID,
        leaf_index,
    }
}

fn params(terms: SwapDvp, surplus: [u64; 2]) -> SettleDvpProofInputParams {
    let [delivery_surplus, payment_surplus] = surplus;
    SettleDvpProofInputParams {
        settlement_authority: terms.settlement_authority,
        current_timestamp: NOW,
        dvp_utxo: DvpUtxo::new(terms.clone(), DVP_UTXO_TREE_ID),
        dvp_utxo_leaf_index: 2,
        delivery_deposit: deposit(&terms, Leg::Delivery, delivery_surplus),
        payment_deposit: deposit(&terms, Leg::Payment, payment_surplus),
        output_tree_id: OUTPUT_TREE_ID,
        terms,
    }
}

fn sdk_inputs(terms: SwapDvp, surplus: [u64; 2]) -> SettleDvpProofInputs {
    params(terms, surplus)
        .to_proof_inputs()
        .expect("sdk proof inputs")
}

/// Sets the four output blindings and the private tx blinding to the ones
/// derived from the settlement seed and `first_nullifier`.
fn derive_blindings(
    inputs: &mut SettleDvpProofInputs,
    terms: &SwapDvp,
    first_nullifier: &[u8; 32],
) {
    let settle_seed = poseidon(&[&u64_right_align(u64::from(SETTLEMENT_TAG)), &terms.blinding])
        .expect("settle seed");
    let seed = derive_output_blinding_seed(first_nullifier, &settle_seed).expect("output seed");
    for (index, output) in (0u32..).zip([
        &mut inputs.buyer_settlement,
        &mut inputs.seller_settlement,
        &mut inputs.seller_refund,
        &mut inputs.buyer_refund,
    ]) {
        output.blinding =
            derive_transact_output_blinding(first_nullifier, &seed, index).expect("blinding");
    }
    inputs.private_tx_blinding =
        derive_private_tx_blinding(first_nullifier, &settle_seed).expect("private tx blinding");
}

/// The settle witness of `terms` with deposits holding the leg amounts plus
/// `surplus`, derived independently of `SettleDvpProofInputParams` and
/// without its checks, so the negative tests can hand the circuit values the
/// sdk would refuse.
fn derive_settle_inputs(terms: &SwapDvp, surplus: [u64; 2]) -> SettleDvpProofInputs {
    let [delivery_surplus, payment_surplus] = surplus;
    let dvp_id = terms.dvp_id().expect("dvp_id");
    let (dvp_authority, _) = terms.dvp_authority().expect("dvp authority");
    let owner_hash = pda_owner_hash(&dvp_authority);
    let dvp_blinding = dvp_utxo_blinding(terms, &owner_hash, &dvp_id);
    let dvp_utxo = ProofInputUtxo::new(owner_hash, &SOL_MINT, 0, &dvp_blinding, DVP_UTXO_TREE_ID)
        .expect("dvp utxo")
        .with_data_hash(dvp_id);
    let first_nullifier = poseidon(&[&dvp_utxo.hash().expect("dvp hash"), &dvp_blinding, &[0; 32]])
        .expect("first nullifier");

    let delivery = terms.delivery_mint;
    let payment = terms.payment_mint;
    let input = |mint: &Address, amount: u64, blinding: [u8; 32]| {
        ProofInputUtxo::new(owner_hash, mint, amount, &blinding, DEPOSIT_TREE_ID).expect("deposit")
    };
    // Blindings are set by `derive_blindings` below.
    let output = |owner: &zolana_keypair::ShieldedAddress, mint: &Address, amount: u64| {
        let owner_hash = owner.owner_hash().expect("destination owner hash");
        ProofInputUtxo::new(owner_hash, mint, amount, &[0; 32], OUTPUT_TREE_ID).expect("output")
    };
    let mut inputs = SettleDvpProofInputs {
        public_input_hash: [0u8; 32],
        private_tx_hash: [0u8; 32],
        creator_id: solana_owner_identity(terms.creator.as_array()).expect("creator id"),
        dvp_id,
        dvp_authority_owner_hash: owner_hash,
        settlement_authority_id: solana_owner_identity(terms.settlement_authority.as_array())
            .expect("settlement authority id"),
        current_timestamp: NOW as u64,
        terms: SwapDvpProofInput::try_from(terms).expect("terms proof input"),
        dvp_utxo,
        delivery_deposit: input(&delivery, terms.delivery_amount + delivery_surplus, fe(21)),
        payment_deposit: input(&payment, terms.payment_amount + payment_surplus, fe(22)),
        buyer_settlement: output(
            &terms.buyer_settlement_destination,
            &delivery,
            terms.delivery_amount,
        ),
        seller_settlement: output(
            &terms.seller_settlement_destination,
            &payment,
            terms.payment_amount,
        ),
        seller_refund: output(
            &terms.seller_refund_destination,
            &delivery,
            delivery_surplus,
        ),
        buyer_refund: output(&terms.buyer_refund_destination, &payment, payment_surplus),
        private_tx_blinding: [0u8; 32],
    };
    derive_blindings(&mut inputs, terms, &first_nullifier);
    reseal(&mut inputs);
    inputs
}

/// Recomputes the private tx hash and the public input hash from the other
/// fields. A negative test tampers one field and reseals, so the proof fails
/// on exactly that field and not on a stale hash.
fn reseal(inputs: &mut SettleDvpProofInputs) {
    let hash = |utxo: &ProofInputUtxo| utxo.hash().expect("utxo hash");
    inputs.private_tx_hash = PrivateTxHash::new(
        &[
            hash(&inputs.dvp_utxo),
            hash(&inputs.delivery_deposit),
            hash(&inputs.payment_deposit),
        ],
        &[
            hash(&inputs.buyer_settlement),
            hash(&inputs.seller_settlement),
            hash(&inputs.seller_refund),
            hash(&inputs.buyer_refund),
        ],
        &inputs.private_tx_blinding,
    )
    .hash()
    .expect("private tx hash");
    inputs.public_input_hash = SettleDvpPublicInput {
        private_tx_hash: &inputs.private_tx_hash,
        creator_id: &inputs.creator_id,
        dvp_id: &inputs.dvp_id,
        dvp_authority_owner_hash: &inputs.dvp_authority_owner_hash,
        settlement_authority_id: &inputs.settlement_authority_id,
        current_timestamp: inputs.current_timestamp as i64,
    }
    .hash()
    .expect("public input hash");
}

/// Derives the surplus witness of the sample terms, tampers it, reseals it,
/// and asserts the circuit refuses to prove it.
fn assert_unprovable(reason: &str, tamper: impl FnOnce(&mut SettleDvpProofInputs)) {
    ensure_keys(CIRCUIT);
    let mut inputs = derive_settle_inputs(&sample_terms(), SURPLUS);
    tamper(&mut inputs);
    reseal(&mut inputs);
    assert_unsatisfied(inputs.prove(), reason);
}

/// Asserts the sdk refuses the tampered surplus parameters.
fn assert_sdk_rejects(reason: &str, tamper: impl FnOnce(&mut SettleDvpProofInputParams)) {
    let mut params = params(sample_terms(), SURPLUS);
    tamper(&mut params);
    assert!(
        params.to_proof_inputs().is_err(),
        "the sdk must reject: {reason}"
    );
}

fn prove_and_verify(inputs: &SettleDvpProofInputs) {
    let proof = inputs.prove().expect("prove failed");
    assert_verifies(CIRCUIT, &proof, inputs.public_input_hash, &VERIFYINGKEY);
}

fn without_earliest() -> SwapDvp {
    SwapDvp {
        earliest_settlement_timestamp: None,
        ..sample_terms()
    }
}

#[test]
fn program_vk_has_no_commitment() {
    assert_single_input_vk(&VERIFYINGKEY);
}

/// The sdk derivation matches the independent one, with and without
/// surplus, so the negative tests below start from the witness the sdk would
/// build. The SPP outputs the sdk derives carry the same blindings.
#[test]
fn sdk_inputs_match_independent_derivation() {
    for surplus in [EXACT, SURPLUS] {
        let derivation = params(sample_terms(), surplus).derive().expect("derive");
        let expected = derive_settle_inputs(&sample_terms(), surplus);
        assert_eq!(
            derivation
                .outputs
                .iter()
                .map(|output| output.blinding)
                .collect::<Vec<_>>(),
            [
                expected.buyer_settlement.blinding,
                expected.seller_settlement.blinding,
                expected.seller_refund.blinding,
                expected.buyer_refund.blinding,
            ],
            "{surplus:?}"
        );
        assert_eq!(derivation.proof_inputs, expected, "{surplus:?}");
    }
}

/// Exact funding: both change outputs are zero-amount UTXOs (D2).
#[test]
fn settles_exact_funding() {
    ensure_keys(CIRCUIT);
    let inputs = sdk_inputs(sample_terms(), EXACT);
    assert_eq!(
        (inputs.seller_refund.amount, inputs.buyer_refund.amount),
        ([0u8; 32], [0u8; 32])
    );
    prove_and_verify(&inputs);
}

#[test]
fn settles_with_surplus() {
    ensure_keys(CIRCUIT);
    prove_and_verify(&sdk_inputs(sample_terms(), SURPLUS));
}

/// The window is inclusive at both ends.
#[test]
fn settles_at_the_window_bounds() {
    ensure_keys(CIRCUIT);
    let terms = sample_terms();
    let earliest = terms
        .earliest_settlement_timestamp
        .expect("sample terms set an earliest settlement timestamp");
    for current_timestamp in [earliest, terms.expiry_timestamp] {
        let inputs = SettleDvpProofInputParams {
            current_timestamp,
            ..params(terms.clone(), SURPLUS)
        }
        .to_proof_inputs()
        .expect("sdk proof inputs");
        prove_and_verify(&inputs);
    }
}

/// Without an earliest settlement timestamp, settling long before the sample
/// terms' earliest timestamp is fine.
#[test]
fn settles_without_earliest() {
    ensure_keys(CIRCUIT);
    let inputs = SettleDvpProofInputParams {
        current_timestamp: 1_000,
        ..params(without_earliest(), EXACT)
    }
    .to_proof_inputs()
    .expect("sdk proof inputs");
    prove_and_verify(&inputs);
}

#[test]
fn settle_dvp_rejects_tampered_public_input() {
    ensure_keys(CIRCUIT);
    let inputs = sdk_inputs(sample_terms(), SURPLUS);
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
fn settle_dvp_rejects_wrong_authority() {
    let buyer = sample_terms().buyer;
    assert_sdk_rejects("the signer is not the settlement authority", |params| {
        params.settlement_authority = buyer
    });
    let buyer_id = solana_owner_identity(buyer.as_array()).expect("buyer id");
    assert_unprovable("the signer is not the settlement authority", |inputs| {
        inputs.settlement_authority_id = buyer_id
    });
}

#[test]
fn settle_dvp_rejects_after_expiry() {
    let after = sample_terms().expiry_timestamp + 1;
    assert_sdk_rejects("the DvP has expired", |params| {
        params.current_timestamp = after
    });
    assert_unprovable("the DvP has expired", |inputs| {
        inputs.current_timestamp = after as u64
    });
}

#[test]
fn settle_dvp_rejects_before_earliest() {
    let before = sample_terms()
        .earliest_settlement_timestamp
        .expect("sample terms set an earliest settlement timestamp")
        - 1;
    assert_sdk_rejects("before the earliest settlement timestamp", |params| {
        params.current_timestamp = before
    });
    assert_unprovable("before the earliest settlement timestamp", |inputs| {
        inputs.current_timestamp = before as u64
    });
}

#[test]
fn settle_dvp_rejects_underfunded_deposit() {
    assert_sdk_rejects("the delivery deposit is below the leg amount", |params| {
        params.delivery_deposit.amount = params.terms.delivery_amount - 1
    });
    assert_sdk_rejects("the payment deposit is below the leg amount", |params| {
        params.payment_deposit.amount = params.terms.payment_amount - 1
    });
    // The change output carries in - leg = -1 in the field, so only the
    // deposit amount check can fail.
    let short = sample_terms().delivery_amount - 1;
    assert_unprovable("the delivery deposit is below the leg amount", |inputs| {
        inputs.delivery_deposit.amount = u64_right_align(short);
        inputs.seller_refund.amount = MINUS_ONE;
    });
}

#[test]
fn settle_dvp_rejects_mismatched_deposits() {
    assert_sdk_rejects("the deposits are swapped", |params| {
        std::mem::swap(&mut params.delivery_deposit, &mut params.payment_deposit)
    });
    assert_sdk_rejects("the delivery deposit is the payment mint", |params| {
        params.delivery_deposit.mint = Mint::SOL
    });
    assert_sdk_rejects("the deposit belongs to other terms", |params| {
        params.payment_deposit.terms = without_earliest()
    });
}

#[test]
fn settle_dvp_rejects_swapped_destination() {
    // The buyer's delivery goes to the seller's settlement destination.
    assert_unprovable("delivery to the seller", |inputs| {
        inputs.buyer_settlement.owner_hash = inputs.seller_settlement.owner_hash
    });
}

#[test]
fn settle_dvp_rejects_wrong_change_amount() {
    let [delivery_surplus, _] = SURPLUS;
    assert_unprovable("the seller's change is short", |inputs| {
        inputs.seller_refund.amount = u64_right_align(delivery_surplus - 1)
    });
}

#[test]
fn settle_dvp_rejects_non_derived_output_blinding() {
    assert_unprovable("the buyer settlement blinding is random", |inputs| {
        inputs.buyer_settlement.blinding = fe(9)
    });
}

/// Every blinding derived from the delivery deposit's nullifier instead of
/// the DvP UTXO's: a consistent derivation, but from the wrong first
/// nullifier.
#[test]
fn settle_dvp_rejects_wrong_first_nullifier() {
    let terms = sample_terms();
    let deposit = deposit(&terms, Leg::Delivery, 17);
    let wrong_nullifier = deposit.to_input_utxo().expect("delivery input").nullifier;
    assert_unprovable("blindings from the wrong first nullifier", |inputs| {
        derive_blindings(inputs, &terms, &wrong_nullifier)
    });
}

/// A different DvP UTXO blinding changes the first nullifier, so the output
/// blindings no longer match.
#[test]
fn settle_dvp_rejects_wrong_dvp_utxo_blinding() {
    assert_unprovable("the DvP UTXO blinding is not the created one", |inputs| {
        inputs.dvp_utxo.blinding = fe(9)
    });
}

#[test]
fn settle_dvp_rejects_deposit_with_wrong_owner() {
    assert_unprovable("the deposit is not owned by the DvP authority", |inputs| {
        inputs.payment_deposit.owner_hash = fe(9)
    });
}
