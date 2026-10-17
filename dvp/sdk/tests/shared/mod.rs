use dvp_program::circuits::*;
use dvp_sdk::{
    proof::CircuitId,
    state::{Deposit, DvpUtxo, Leg, SwapDvp},
};
use solana_address::Address;
use zolana_hasher::primitives::solana_owner_identity;
use zolana_keypair::{NullifierKey, PublicKey, ShieldedAddress, ViewingKey};
use zolana_program::{ProgramOwner, TxContext, ZkProgram};
use zolana_transaction::{Mint, WalletUtxo, SOL_MINT};

pub const OUTPUT_TREE_ID: u16 = 7;
pub const NOW: u64 = 1_750_000_000;

pub fn create_inputs(terms: &SwapDvp) -> CreateDvp {
    CreateDvp {
        private: CreateDvpPrivateInputs {
            tx_context: context(terms.create_blinding_seed().unwrap()),
            terms: terms.try_into().unwrap(),
        },
        public: CreateDvpPublicInputs {
            creator_identity: identity(terms.creator),
            dvp_id: terms.dvp_id().unwrap(),
            dvp_owner: owner(terms),
        },
    }
}

pub fn settle_inputs(surplus: [u64; 2]) -> SettleDvp {
    let terms = sample_terms();
    SettleDvp {
        private: SettleDvpPrivateInputs {
            tx_context: context(terms.settle_blinding_seed().unwrap()),
            terms: terms.circuit_terms(delivery_mint(), Mint::SOL).unwrap(),
            dvp_utxo: DvpUtxo::new(terms.clone(), 3).to_input_utxo(2).unwrap(),
            delivery_deposit: deposit(&terms, Leg::Delivery, terms.delivery_amount + surplus[0])
                .to_input_utxo()
                .unwrap(),
            payment_deposit: deposit(&terms, Leg::Payment, terms.payment_amount + surplus[1])
                .to_input_utxo()
                .unwrap(),
        },
        public: SettleDvpPublicInputs {
            creator_identity: identity(terms.creator),
            dvp_id: terms.dvp_id().unwrap(),
            dvp_owner: owner(&terms),
            settlement_authority_identity: identity(terms.settlement_authority),
            current_timestamp: NOW,
        },
    }
}

pub fn reclaim_inputs(payment: bool) -> ReclaimDvp {
    let terms = sample_terms();
    let leg = if payment { Leg::Payment } else { Leg::Delivery };
    ReclaimDvp {
        private: ReclaimDvpPrivateInputs {
            tx_context: context(fe(41)),
            terms: terms.circuit_terms(delivery_mint(), Mint::SOL).unwrap(),
            deposit: deposit(&terms, leg, leg.amount(&terms) + 17)
                .to_input_utxo()
                .unwrap(),
            payment_leg: payment,
            refund_destination: if payment {
                terms.buyer_refund_destination
            } else {
                terms.seller_refund_destination
            },
        },
        public: ReclaimDvpPublicInputs {
            creator_identity: identity(terms.creator),
            dvp_id: terms.dvp_id().unwrap(),
            dvp_owner: owner(&terms),
            signer_identity: identity(if payment { terms.buyer } else { terms.seller }),
        },
    }
}

pub fn abort_inputs(reject: bool, buyer: bool, delivery: bool, payment: bool) -> AbortDvp {
    let terms = sample_terms();
    AbortDvp {
        private: AbortDvpPrivateInputs {
            tx_context: context(terms.abort_blinding_seed().unwrap()),
            terms: terms.circuit_terms(delivery_mint(), Mint::SOL).unwrap(),
            dvp_utxo: DvpUtxo::new(terms.clone(), 3).to_input_utxo(2).unwrap(),
            delivery_deposit: if delivery {
                deposit(&terms, Leg::Delivery, 1_017)
                    .to_input_utxo()
                    .unwrap()
            } else {
                WalletUtxo::dummy(4).unwrap()
            },
            payment_deposit: if payment {
                deposit(&terms, Leg::Payment, 2_499_999_995)
                    .to_input_utxo()
                    .unwrap()
            } else {
                WalletUtxo::dummy(4).unwrap()
            },
            buyer_rejects: buyer,
        },
        public: AbortDvpPublicInputs {
            creator_identity: identity(terms.creator),
            dvp_id: terms.dvp_id().unwrap(),
            dvp_owner: owner(&terms),
            signer_identity: identity(if reject {
                if buyer {
                    terms.buyer
                } else {
                    terms.seller
                }
            } else {
                terms.settlement_authority
            }),
            reject,
        },
    }
}

pub fn prove_and_verify<P: ZkProgram>(
    id: CircuitId,
    inputs: &P,
    vk: &groth16_solana::groth16::Groth16Verifyingkey,
) {
    let tx = inputs
        .create_program_transaction(&destination(91), Address::default())
        .unwrap();
    let proof = dvp_sdk::proof::prove(id, inputs).unwrap();
    assert!(proof.a.iter().any(|&byte| byte != 0));
    let a = groth16_solana::decompression::decompress_g1(&proof.a).unwrap();
    let b = groth16_solana::decompression::decompress_g2(&proof.b).unwrap();
    let c = groth16_solana::decompression::decompress_g1(&proof.c).unwrap();
    groth16_solana::groth16::Groth16Verifier::new(&a, &b, &c, &[tx.public_hash], vk)
        .unwrap()
        .verify()
        .unwrap();
    let mut tampered = tx.public_hash;
    tampered[31] ^= 1;
    assert!(
        groth16_solana::groth16::Groth16Verifier::new(&a, &b, &c, &[tampered], vk)
            .unwrap()
            .verify()
            .is_err()
    );
}

pub fn assert_invalid<P: ZkProgram>(id: CircuitId, inputs: &P) {
    assert!(inputs
        .create_finalized_transaction(&destination(91), Address::default())
        .is_err());
    let error = dvp_sdk::proof::prove(id, inputs).expect_err("invalid witness must be unprovable");
    assert!(
        error
            .downcast_ref::<zolana_program::CircuitError>()
            .is_some()
            || error
                .downcast_ref::<zolana_program::ProverError>()
                .is_some_and(|error| matches!(
                    error.kind(),
                    zolana_program::ProverErrorKind::ProofInputsBreakRule(_)
                        | zolana_program::ProverErrorKind::Circuit(_)
                )),
        "expected a constraint failure, received {error}"
    );
}

pub fn context(seed: [u8; 32]) -> TxContext {
    TxContext::new()
        .with_blinding_seed(seed)
        .with_output_tree_id(Some(OUTPUT_TREE_ID))
}
pub fn identity(address: Address) -> [u8; 32] {
    solana_owner_identity(address.as_array()).unwrap()
}
pub fn fe(value: u8) -> [u8; 32] {
    let mut bytes = [0; 32];
    bytes[31] = value;
    bytes
}
pub fn owner(terms: &SwapDvp) -> ShieldedAddress {
    ProgramOwner::new(terms.dvp_authority().unwrap().0)
        .address(ViewingKey::new().pubkey())
        .unwrap()
}
pub fn delivery_mint() -> Mint {
    Mint::new(Address::new_from_array([3; 32]), 2)
}
pub fn deposit(terms: &SwapDvp, leg: Leg, amount: u64) -> Deposit {
    Deposit {
        terms: terms.clone(),
        leg,
        mint: if leg == Leg::Delivery {
            delivery_mint()
        } else {
            Mint::SOL
        },
        amount,
        blinding: fe(if leg == Leg::Delivery { 21 } else { 22 }),
        tree_id: 4,
        leaf_index: if leg == Leg::Delivery { 5 } else { 6 },
    }
}
pub fn destination(seed: u8) -> ShieldedAddress {
    ShieldedAddress {
        signing_pubkey: PublicKey::from_ed25519(&[seed; 32]),
        nullifier_pubkey: NullifierKey::from_secret([seed; 31]).pubkey().unwrap(),
        viewing_pubkey: ViewingKey::from_bytes(&[seed; 32]).unwrap().pubkey(),
    }
}
pub fn sample_terms() -> SwapDvp {
    SwapDvp {
        seller: Address::new_from_array([1; 32]),
        buyer: Address::new_from_array([2; 32]),
        delivery_mint: delivery_mint().asset,
        payment_mint: SOL_MINT,
        settlement_authority: Address::new_from_array([4; 32]),
        creator: Address::new_from_array([5; 32]),
        delivery_amount: 1_000,
        payment_amount: 2_500_000_000,
        expiry_timestamp: 1_800_000_000,
        earliest_settlement_timestamp: Some(1_700_000_000),
        nonce: 7,
        ref_string: [0; 64],
        blinding: fe(77),
        seller_settlement_destination: destination(11),
        buyer_settlement_destination: destination(12),
        seller_refund_destination: destination(13),
        buyer_refund_destination: destination(14),
    }
}
