use dvp_program::{
    instructions::{abort_dvp::AbortDvpPublicInput, shared::u64_right_align},
    verifying_keys::abort_dvp::VERIFYINGKEY,
    ABORT_TAG,
};
use dvp_prover::{AbortDvpProofInputs, CircuitId, ProofInputUtxo, SwapDvpProofInput};
use dvp_sdk::{
    instructions::abort_dvp::{AbortDvpProofInputParams, AbortMode},
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

const CIRCUIT: CircuitId = CircuitId::AbortDvp;

/// The tree the DvP UTXO was appended to.
const DVP_UTXO_TREE_ID: u16 = 3;
/// The tree the deposits and the padding are spent from. Both input trees
/// differ from the output tree, so a swapped tree id changes a commitment the
/// proof binds.
const DEPOSIT_TREE_ID: u16 = 4;
/// Pool asset id of the sample delivery mint; only the mint enters the proof.
const DELIVERY_ASSET_ID: u64 = 2;
/// Deposited amounts: an abort refunds whatever a funder sent, so the
/// delivery deposit holds a surplus and the payment deposit falls short.
const DELIVERY_DEPOSIT: u64 = 1_017;
const PAYMENT_DEPOSIT: u64 = 2_499_999_995;

/// Which deposits exist when the DvP is aborted, by leg.
#[derive(Clone, Copy, Debug)]
struct Funded {
    delivery: bool,
    payment: bool,
}

const BOTH: Funded = Funded {
    delivery: true,
    payment: true,
};
const DELIVERY_ONLY: Funded = Funded {
    delivery: true,
    payment: false,
};
const PAYMENT_ONLY: Funded = Funded {
    delivery: false,
    payment: true,
};
const NONE: Funded = Funded {
    delivery: false,
    payment: false,
};

/// Who aborts and in which mode.
#[derive(Clone, Copy, Debug)]
enum Aborter {
    /// cancel_dvp by the settlement authority.
    Authority,
    /// reject_dvp by the seller.
    Seller,
    /// reject_dvp by the buyer.
    Buyer,
}

impl Aborter {
    fn mode(self) -> AbortMode {
        match self {
            Self::Authority => AbortMode::Cancel,
            Self::Seller | Self::Buyer => AbortMode::Reject,
        }
    }

    fn signer(self, terms: &SwapDvp) -> Address {
        match self {
            Self::Authority => terms.settlement_authority,
            Self::Seller => terms.seller,
            Self::Buyer => terms.buyer,
        }
    }
}

fn leg_mint(terms: &SwapDvp, leg: Leg) -> Mint {
    match leg {
        Leg::Delivery => Mint::new(terms.delivery_mint, DELIVERY_ASSET_ID),
        Leg::Payment => Mint::SOL,
    }
}

fn deposit(terms: &SwapDvp, leg: Leg) -> Deposit {
    let (amount, blinding, leaf_index) = match leg {
        Leg::Delivery => (DELIVERY_DEPOSIT, fe(21), 5),
        Leg::Payment => (PAYMENT_DEPOSIT, fe(22), 6),
    };
    Deposit {
        terms: terms.clone(),
        leg,
        mint: leg_mint(terms, leg),
        amount,
        blinding,
        tree_id: DEPOSIT_TREE_ID,
        leaf_index,
    }
}

fn params(terms: SwapDvp, aborter: Aborter, funded: Funded) -> AbortDvpProofInputParams {
    AbortDvpProofInputParams {
        signer: aborter.signer(&terms),
        mode: aborter.mode(),
        dvp_utxo: DvpUtxo::new(terms.clone(), DVP_UTXO_TREE_ID),
        dvp_utxo_leaf_index: 2,
        delivery_mint: leg_mint(&terms, Leg::Delivery),
        payment_mint: leg_mint(&terms, Leg::Payment),
        delivery_deposit: funded.delivery.then(|| deposit(&terms, Leg::Delivery)),
        payment_deposit: funded.payment.then(|| deposit(&terms, Leg::Payment)),
        padding_tree_id: DEPOSIT_TREE_ID,
        output_tree_id: OUTPUT_TREE_ID,
        terms,
    }
}

fn sdk_inputs(aborter: Aborter, funded: Funded) -> AbortDvpProofInputs {
    params(sample_terms(), aborter, funded)
        .to_proof_inputs()
        .expect("sdk proof inputs")
}

/// The abort witness of the sample terms, derived independently of
/// `AbortDvpProofInputParams` and without its checks, so the negative tests
/// can hand the circuit values the sdk would refuse. Deposits fill the input
/// slots first; a padding slot gets the all-zero witness, as in the sdk.
fn derive_abort_inputs(aborter: Aborter, funded: Funded) -> AbortDvpProofInputs {
    let terms = &sample_terms();
    let dvp_id = terms.dvp_id().expect("dvp_id");
    let (dvp_authority, _) = terms.dvp_authority().expect("dvp authority");
    let owner_hash = pda_owner_hash(&dvp_authority);
    let dvp_blinding = dvp_utxo_blinding(terms, &owner_hash, &dvp_id);
    let dvp_utxo = ProofInputUtxo::new(owner_hash, &SOL_MINT, 0, &dvp_blinding, DVP_UTXO_TREE_ID)
        .expect("dvp utxo")
        .with_data_hash(dvp_id);
    let first_nullifier = poseidon(&[&dvp_utxo.hash().expect("dvp hash"), &dvp_blinding, &[0; 32]])
        .expect("first nullifier");

    let input = |mint: &Address, amount: u64, blinding: [u8; 32]| {
        ProofInputUtxo::new(owner_hash, mint, amount, &blinding, DEPOSIT_TREE_ID).expect("deposit")
    };
    let delivery_amount = funded.delivery.then_some(DELIVERY_DEPOSIT);
    let payment_amount = funded.payment.then_some(PAYMENT_DEPOSIT);
    let delivery = delivery_amount.map(|amount| input(&terms.delivery_mint, amount, fe(21)));
    let payment = payment_amount.map(|amount| input(&terms.payment_mint, amount, fe(22)));
    let mut slots = delivery.into_iter().chain(payment);
    let slot1 = slots.next();
    let slot2 = slots.next();

    let abort_seed =
        poseidon(&[&u64_right_align(u64::from(ABORT_TAG)), &terms.blinding]).expect("abort seed");
    let seed = derive_output_blinding_seed(&first_nullifier, &abort_seed).expect("output seed");
    let output = |index: u32,
                  owner: &zolana_keypair::ShieldedAddress,
                  mint: &Address,
                  amount: Option<u64>| {
        let owner_hash = owner.owner_hash().expect("destination owner hash");
        let blinding =
            derive_transact_output_blinding(&first_nullifier, &seed, index).expect("blinding");
        ProofInputUtxo::new(
            owner_hash,
            mint,
            amount.unwrap_or(0),
            &blinding,
            OUTPUT_TREE_ID,
        )
        .expect("refund")
    };
    let signer = aborter.signer(terms);
    let mut inputs = AbortDvpProofInputs {
        public_input_hash: [0u8; 32],
        private_tx_hash: [0u8; 32],
        creator_id: solana_owner_identity(terms.creator.as_array()).expect("creator id"),
        dvp_id,
        dvp_authority_owner_hash: owner_hash,
        signer_id: solana_owner_identity(signer.as_array()).expect("signer id"),
        mode: aborter.mode(),
        terms: SwapDvpProofInput::try_from(terms).expect("terms proof input"),
        leg: matches!(aborter, Aborter::Buyer),
        payment_first: !funded.delivery && funded.payment,
        padding1: slot1.is_none(),
        padding2: slot2.is_none(),
        dvp_utxo,
        slot1: slot1.unwrap_or_default(),
        slot2: slot2.unwrap_or_default(),
        seller_refund: output(
            0,
            &terms.seller_refund_destination,
            &terms.delivery_mint,
            delivery_amount,
        ),
        buyer_refund: output(
            1,
            &terms.buyer_refund_destination,
            &terms.payment_mint,
            payment_amount,
        ),
        private_tx_blinding: derive_private_tx_blinding(&first_nullifier, &abort_seed)
            .expect("private tx blinding"),
    };
    reseal(&mut inputs);
    inputs
}

/// Recomputes the private tx hash (a padding slot contributes 0) and the
/// public input hash from the other fields. A negative test tampers one
/// field and reseals, so the proof fails on exactly that field and not on a
/// stale hash.
fn reseal(inputs: &mut AbortDvpProofInputs) {
    let hash = |utxo: &ProofInputUtxo| utxo.hash().expect("utxo hash");
    let slot_hash = |utxo: &ProofInputUtxo, padding: bool| {
        if padding {
            [0u8; 32]
        } else {
            hash(utxo)
        }
    };
    inputs.private_tx_hash = PrivateTxHash::new(
        &[
            hash(&inputs.dvp_utxo),
            slot_hash(&inputs.slot1, inputs.padding1),
            slot_hash(&inputs.slot2, inputs.padding2),
        ],
        &[hash(&inputs.seller_refund), hash(&inputs.buyer_refund)],
        &inputs.private_tx_blinding,
    )
    .hash()
    .expect("private tx hash");
    inputs.public_input_hash = AbortDvpPublicInput {
        private_tx_hash: &inputs.private_tx_hash,
        creator_id: &inputs.creator_id,
        dvp_id: &inputs.dvp_id,
        dvp_authority_owner_hash: &inputs.dvp_authority_owner_hash,
        signer_id: &inputs.signer_id,
        mode: inputs.mode.value(),
    }
    .hash()
    .expect("public input hash");
}

/// Derives the witness, tampers it, reseals it, and asserts the circuit
/// refuses to prove it.
fn assert_unprovable(
    aborter: Aborter,
    funded: Funded,
    reason: &str,
    tamper: impl FnOnce(&mut AbortDvpProofInputs),
) {
    ensure_keys(CIRCUIT);
    let mut inputs = derive_abort_inputs(aborter, funded);
    tamper(&mut inputs);
    reseal(&mut inputs);
    assert_unsatisfied(inputs.prove(), reason);
}

/// Asserts the sdk refuses the tampered parameters.
fn assert_sdk_rejects(
    aborter: Aborter,
    funded: Funded,
    reason: &str,
    tamper: impl FnOnce(&mut AbortDvpProofInputParams),
) {
    let mut params = params(sample_terms(), aborter, funded);
    tamper(&mut params);
    assert!(
        params.to_proof_inputs().is_err(),
        "the sdk must reject: {reason}"
    );
}

fn prove_and_verify(inputs: &AbortDvpProofInputs) {
    let proof = inputs.prove().expect("prove failed");
    assert_verifies(CIRCUIT, &proof, inputs.public_input_hash, &VERIFYINGKEY);
}

/// The slot layout and refund amounts of a witness:
/// `(payment_first, padding1, padding2, seller refund, buyer refund)`.
fn layout(inputs: &AbortDvpProofInputs) -> (bool, bool, bool, [u8; 32], [u8; 32]) {
    (
        inputs.payment_first,
        inputs.padding1,
        inputs.padding2,
        inputs.seller_refund.amount,
        inputs.buyer_refund.amount,
    )
}

#[test]
fn program_vk_has_no_commitment() {
    assert_single_input_vk(&VERIFYINGKEY);
}

/// The sdk derivation matches the independent one for every funding state,
/// so the negative tests below start from the witness the sdk would build.
/// The SPP outputs carry the same blindings, and the SPP inputs hold the
/// deposits first and random padding last.
#[test]
fn sdk_inputs_match_independent_derivation() {
    for (aborter, funded) in [
        (Aborter::Authority, BOTH),
        (Aborter::Authority, DELIVERY_ONLY),
        (Aborter::Buyer, PAYMENT_ONLY),
        (Aborter::Seller, NONE),
    ] {
        let derivation = params(sample_terms(), aborter, funded)
            .derive()
            .expect("derive");
        let expected = derive_abort_inputs(aborter, funded);
        assert_eq!(
            derivation
                .outputs
                .iter()
                .map(|output| output.blinding)
                .collect::<Vec<_>>(),
            [
                expected.seller_refund.blinding,
                expected.buyer_refund.blinding
            ],
            "{aborter:?} {funded:?}"
        );
        assert_eq!(
            derivation
                .inputs
                .iter()
                .map(|input| (input.is_dummy(), input.compact))
                .collect::<Vec<_>>(),
            [
                (false, false),
                (expected.padding1, false),
                (expected.padding2, false)
            ],
            "{aborter:?} {funded:?}"
        );
        assert_eq!(derivation.proof_inputs, expected, "{aborter:?} {funded:?}");
    }
}

#[test]
fn cancels_with_both_deposits() {
    ensure_keys(CIRCUIT);
    let inputs = sdk_inputs(Aborter::Authority, BOTH);
    assert_eq!(
        layout(&inputs),
        (
            false,
            false,
            false,
            u64_right_align(DELIVERY_DEPOSIT),
            u64_right_align(PAYMENT_DEPOSIT)
        )
    );
    prove_and_verify(&inputs);
}

/// The payment leg's refund is a zero-amount UTXO.
#[test]
fn cancels_with_delivery_only() {
    ensure_keys(CIRCUIT);
    let inputs = sdk_inputs(Aborter::Authority, DELIVERY_ONLY);
    assert_eq!(
        layout(&inputs),
        (
            false,
            false,
            true,
            u64_right_align(DELIVERY_DEPOSIT),
            [0u8; 32]
        )
    );
    prove_and_verify(&inputs);
}

/// The payment deposit fills slot 1 under the leg-order bit.
#[test]
fn cancels_with_payment_only() {
    ensure_keys(CIRCUIT);
    let inputs = sdk_inputs(Aborter::Authority, PAYMENT_ONLY);
    assert_eq!(
        layout(&inputs),
        (
            true,
            false,
            true,
            [0u8; 32],
            u64_right_align(PAYMENT_DEPOSIT)
        )
    );
    prove_and_verify(&inputs);
}

#[test]
fn cancels_without_deposits() {
    ensure_keys(CIRCUIT);
    let inputs = sdk_inputs(Aborter::Authority, NONE);
    assert_eq!(layout(&inputs), (false, true, true, [0u8; 32], [0u8; 32]));
    prove_and_verify(&inputs);
}

#[test]
fn rejects_by_the_seller() {
    ensure_keys(CIRCUIT);
    let inputs = sdk_inputs(Aborter::Seller, DELIVERY_ONLY);
    assert_eq!((inputs.mode, inputs.leg), (AbortMode::Reject, false));
    prove_and_verify(&inputs);
}

#[test]
fn rejects_by_the_buyer() {
    ensure_keys(CIRCUIT);
    let inputs = sdk_inputs(Aborter::Buyer, PAYMENT_ONLY);
    assert_eq!((inputs.mode, inputs.leg), (AbortMode::Reject, true));
    prove_and_verify(&inputs);
}

/// With both deposits the leg-order bit may take either value: the payment
/// deposit in slot 1 proves as well.
#[test]
fn aborts_with_both_deposits_payment_first() {
    ensure_keys(CIRCUIT);
    let mut inputs = derive_abort_inputs(Aborter::Authority, BOTH);
    std::mem::swap(&mut inputs.slot1, &mut inputs.slot2);
    inputs.payment_first = true;
    reseal(&mut inputs);
    prove_and_verify(&inputs);
}

#[test]
fn abort_dvp_rejects_tampered_public_input() {
    ensure_keys(CIRCUIT);
    let inputs = sdk_inputs(Aborter::Authority, BOTH);
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

/// In reject mode the settlement authority is no party, under either leg
/// bit.
#[test]
fn abort_dvp_rejects_non_party_signer_in_reject_mode() {
    let authority = sample_terms().settlement_authority;
    assert_sdk_rejects(Aborter::Seller, BOTH, "the authority rejects", |params| {
        params.signer = authority
    });
    let authority_id = solana_owner_identity(authority.as_array()).expect("authority id");
    for leg in [false, true] {
        assert_unprovable(Aborter::Seller, BOTH, "the authority rejects", |inputs| {
            inputs.signer_id = authority_id;
            inputs.leg = leg;
        });
    }
}

#[test]
fn abort_dvp_rejects_party_signer_in_cancel_mode() {
    let seller = sample_terms().seller;
    assert_sdk_rejects(Aborter::Authority, BOTH, "the seller cancels", |params| {
        params.signer = seller
    });
    let seller_id = solana_owner_identity(seller.as_array()).expect("seller id");
    assert_unprovable(Aborter::Authority, BOTH, "the seller cancels", |inputs| {
        inputs.signer_id = seller_id
    });
}

/// The delivery deposit moved behind a padding slot 1. The leg-order bit
/// keeps its asset and refund consistent, and the private tx hash skips the
/// padding either way, so only the padding-last rule refuses it.
#[test]
fn abort_dvp_rejects_padding_before_a_deposit() {
    assert_unprovable(
        Aborter::Authority,
        DELIVERY_ONLY,
        "padding before the deposit",
        |inputs| {
            std::mem::swap(&mut inputs.slot1, &mut inputs.slot2);
            inputs.payment_first = true;
            inputs.padding1 = true;
            inputs.padding2 = false;
        },
    );
}

#[test]
fn abort_dvp_rejects_refund_to_wrong_destination() {
    assert_unprovable(
        Aborter::Authority,
        BOTH,
        "the delivery refund goes to the buyer",
        |inputs| inputs.seller_refund.owner_hash = inputs.buyer_refund.owner_hash,
    );
}

#[test]
fn abort_dvp_rejects_partial_refund() {
    assert_unprovable(
        Aborter::Authority,
        BOTH,
        "the delivery refund is short",
        |inputs| inputs.seller_refund.amount = u64_right_align(DELIVERY_DEPOSIT - 1),
    );
}

/// A deposit in the slot of the other leg: the sdk refuses a payment deposit
/// passed as the delivery deposit, and the circuit refuses a flipped
/// leg-order bit.
#[test]
fn abort_dvp_rejects_deposit_with_wrong_asset_for_its_slot() {
    let terms = sample_terms();
    assert_sdk_rejects(
        Aborter::Authority,
        BOTH,
        "payment deposit as delivery",
        |params| params.delivery_deposit = Some(deposit(&terms, Leg::Payment)),
    );
    assert_sdk_rejects(
        Aborter::Authority,
        BOTH,
        "delivery deposit in the payment mint",
        |params| params.delivery_deposit.as_mut().expect("funded").mint = Mint::SOL,
    );
    assert_unprovable(
        Aborter::Authority,
        BOTH,
        "the leg-order bit is flipped",
        |inputs| inputs.payment_first = true,
    );
}

#[test]
fn abort_dvp_rejects_deposit_of_other_terms() {
    let other = SwapDvp {
        nonce: 8,
        ..sample_terms()
    };
    assert_sdk_rejects(
        Aborter::Authority,
        BOTH,
        "deposit of other terms",
        |params| params.payment_deposit.as_mut().expect("funded").terms = other,
    );
}

/// A different DvP UTXO blinding changes the first nullifier, so the refund
/// blindings no longer match.
#[test]
fn abort_dvp_rejects_wrong_dvp_utxo_blinding() {
    assert_unprovable(
        Aborter::Authority,
        BOTH,
        "the DvP UTXO blinding is not the created one",
        |inputs| inputs.dvp_utxo.blinding = fe(9),
    );
}
