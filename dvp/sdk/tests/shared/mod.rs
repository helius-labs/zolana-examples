use dvp_program::circuits as circuit;
use dvp_sdk::{
    proof::CircuitId,
    state::{Deposit, DvpUtxo, Leg, SwapDvp},
};
use solana_address::Address;
use std::ops::{Deref, DerefMut};
use zolana_hasher::primitives::solana_owner_identity;
use zolana_keypair::{NullifierKey, PublicKey, ShieldedAddress, ViewingKey};
use zolana_program::{
    circuit::CheckedTransaction, from_bytes, to_bytes, ClientInputs, Field, InputRecords,
    ProgramOwner, ProgramProofInputs, ProgramTransaction, TxContext,
};
use zolana_transaction::{
    instructions::transact::FinalizedTransaction, Mint, WalletUtxo, SOL_MINT,
};

pub const OUTPUT_TREE_ID: u16 = 7;
pub const NOW: u64 = 1_750_000_000;

pub trait Execute: zolana_program::xark::Input {
    fn execute(&self) -> Result<CheckedTransaction, zolana_program::CircuitError>;
}

macro_rules! program {
    ($($name:ident),*) => {$(
        impl Execute for circuit::$name {
            fn execute(&self) -> Result<CheckedTransaction, zolana_program::CircuitError> {
                circuit::$name::execute(self)
            }
        }
        pub type $name = Fixture<circuit::$name>;
    )*};
}
program!(CreateDvp, ReclaimDvp, SettleDvp, AbortDvp);

pub struct Fixture<P> {
    program: P,
    records: InputRecords,
}

impl<P> Deref for Fixture<P> {
    type Target = P;
    fn deref(&self) -> &P {
        &self.program
    }
}

impl<P> DerefMut for Fixture<P> {
    fn deref_mut(&mut self) -> &mut P {
        &mut self.program
    }
}

impl<P: Execute> Fixture<P> {
    fn new(program: P, records: InputRecords) -> Self {
        Self { program, records }
    }

    pub fn create_program_transaction(
        &self,
        sender: &ShieldedAddress,
        payer: Address,
    ) -> anyhow::Result<ProgramTransaction> {
        Ok(self.records.create_program_transaction(
            &self.program,
            self.execute()?,
            sender,
            payer,
        )?)
    }

    pub fn create_finalized_transaction(
        &self,
        sender: &ShieldedAddress,
        payer: Address,
    ) -> anyhow::Result<FinalizedTransaction> {
        Ok(self.create_program_transaction(sender, payer)?.finalized)
    }

    pub fn proof_inputs(&self) -> ProgramProofInputs {
        ProgramProofInputs::new(&self.program, *self.execute().unwrap().public_hash())
    }

    pub fn address(&mut self, address: ShieldedAddress) -> zolana_program::ShieldedAddress {
        self.records.address(&address).unwrap()
    }

    pub fn wallet(&mut self, wallet: &WalletUtxo) -> zolana_program::circuit::Utxo {
        self.records.utxo(wallet).unwrap()
    }

    pub fn terms_input(&mut self, terms: &SwapDvp) -> circuit::SwapDvpTerms {
        terms_input(terms, &mut self.records)
    }
}

pub fn terms_input(terms: &SwapDvp, records: &mut InputRecords) -> circuit::SwapDvpTerms {
    circuit::SwapDvpTerms::from_client(
        terms.circuit_terms(delivery_mint(), Mint::SOL).unwrap(),
        records,
    )
    .unwrap()
}

pub fn create_inputs(terms: &SwapDvp) -> CreateDvp {
    let mut records = InputRecords::new();
    let private = circuit::CreateDvpPrivateInputs {
        tx_context: context(terms.create_blinding_seed().unwrap()),
        terms: circuit::SwapDvpTerms::from_client(terms.try_into().unwrap(), &mut records).unwrap(),
    };
    let public = circuit::CreateDvpPublicInputs {
        creator_identity: field_identity(terms.creator),
        dvp_id: from_bytes(&terms.dvp_id().unwrap()).unwrap(),
        dvp_owner: records.address(&owner(terms)).unwrap(),
    };
    Fixture::new(circuit::CreateDvp { private, public }, records)
}

pub fn settle_inputs(surplus: [u64; 2]) -> SettleDvp {
    settle_inputs_with_state(surplus, None)
}

pub fn settle_inputs_with_state(surplus: [u64; 2], state: Option<WalletUtxo>) -> SettleDvp {
    let terms = sample_terms();
    let mut records = InputRecords::new();
    let state = state.unwrap_or_else(|| DvpUtxo::new(terms.clone(), 3).to_input_utxo(2).unwrap());
    let private = circuit::SettleDvpPrivateInputs {
        tx_context: context(terms.settle_blinding_seed().unwrap()),
        terms: terms_input(&terms, &mut records),
        dvp_utxo: records.utxo(&state).unwrap(),
        delivery_deposit: records
            .utxo(
                &deposit(&terms, Leg::Delivery, terms.delivery_amount + surplus[0])
                    .to_input_utxo()
                    .unwrap(),
            )
            .unwrap(),
        payment_deposit: records
            .utxo(
                &deposit(&terms, Leg::Payment, terms.payment_amount + surplus[1])
                    .to_input_utxo()
                    .unwrap(),
            )
            .unwrap(),
    };
    let public = circuit::SettleDvpPublicInputs {
        creator_identity: field_identity(terms.creator),
        dvp_id: from_bytes(&terms.dvp_id().unwrap()).unwrap(),
        dvp_owner: records.address(&owner(&terms)).unwrap(),
        settlement_authority_identity: field_identity(terms.settlement_authority),
        current_timestamp: NOW,
    };
    Fixture::new(circuit::SettleDvp { private, public }, records)
}

pub fn reclaim_inputs(payment: bool) -> ReclaimDvp {
    let terms = sample_terms();
    let leg = if payment { Leg::Payment } else { Leg::Delivery };
    let mut records = InputRecords::new();
    let private = circuit::ReclaimDvpPrivateInputs {
        tx_context: context(fe(41)),
        terms: terms_input(&terms, &mut records),
        deposit: records
            .utxo(
                &deposit(&terms, leg, leg.amount(&terms) + 17)
                    .to_input_utxo()
                    .unwrap(),
            )
            .unwrap(),
        payment_leg: payment,
        refund_destination: records
            .address(&if payment {
                terms.buyer_refund_destination
            } else {
                terms.seller_refund_destination
            })
            .unwrap(),
    };
    let public = circuit::ReclaimDvpPublicInputs {
        creator_identity: field_identity(terms.creator),
        dvp_id: from_bytes(&terms.dvp_id().unwrap()).unwrap(),
        dvp_owner: records.address(&owner(&terms)).unwrap(),
        signer_identity: field_identity(if payment { terms.buyer } else { terms.seller }),
    };
    Fixture::new(circuit::ReclaimDvp { private, public }, records)
}

pub fn abort_inputs(reject: bool, buyer: bool, delivery: bool, payment: bool) -> AbortDvp {
    let terms = sample_terms();
    let mut records = InputRecords::new();
    let private = circuit::AbortDvpPrivateInputs {
        tx_context: context(terms.abort_blinding_seed().unwrap()),
        terms: terms_input(&terms, &mut records),
        dvp_utxo: records
            .utxo(&DvpUtxo::new(terms.clone(), 3).to_input_utxo(2).unwrap())
            .unwrap(),
        delivery_deposit: records
            .utxo(&if delivery {
                deposit(&terms, Leg::Delivery, 1_017)
                    .to_input_utxo()
                    .unwrap()
            } else {
                WalletUtxo::dummy(4).unwrap()
            })
            .unwrap(),
        payment_deposit: records
            .utxo(&if payment {
                deposit(&terms, Leg::Payment, 2_499_999_995)
                    .to_input_utxo()
                    .unwrap()
            } else {
                WalletUtxo::dummy(4).unwrap()
            })
            .unwrap(),
        buyer_rejects: buyer,
    };
    let public = circuit::AbortDvpPublicInputs {
        creator_identity: field_identity(terms.creator),
        dvp_id: from_bytes(&terms.dvp_id().unwrap()).unwrap(),
        dvp_owner: records.address(&owner(&terms)).unwrap(),
        signer_identity: field_identity(if reject {
            if buyer {
                terms.buyer
            } else {
                terms.seller
            }
        } else {
            terms.settlement_authority
        }),
        reject,
    };
    Fixture::new(circuit::AbortDvp { private, public }, records)
}

pub fn prove_and_verify<P: Execute>(
    id: CircuitId,
    inputs: &Fixture<P>,
    vk: &groth16_solana::groth16::Groth16Verifyingkey,
) {
    let tx = inputs
        .create_program_transaction(&destination(91), Address::default())
        .unwrap();
    zolana_program::xark::prover::circuit_at(dvp_sdk::proof::keys_dir().join(id.name()))
        .check_constrained(tx.proof_inputs.clone())
        .unwrap();
    let proof = dvp_sdk::proof::prove(id, &tx.proof_inputs).unwrap();
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

pub fn assert_invalid<P: Execute>(id: CircuitId, inputs: &Fixture<P>) {
    let native = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| inputs.execute()));
    assert!(
        !matches!(native, Ok(Ok(_))),
        "the native circuit accepted invalid inputs"
    );
    let error =
        zolana_program::xark::prover::circuit_at(dvp_sdk::proof::keys_dir().join(id.name()))
            .check(ProgramProofInputs::new(&inputs.program, Field::zero()))
            .expect_err("the compiled circuit must reject invalid inputs");
    assert!(
        error.to_string().contains("in execute"),
        "expected the program's constraint to fail before the outer public-hash check: {error}"
    );
}

pub fn context(seed: [u8; 32]) -> TxContext {
    InputRecords::context(&seed, Some(OUTPUT_TREE_ID)).unwrap()
}
pub fn identity(address: Address) -> [u8; 32] {
    solana_owner_identity(address.as_array()).unwrap()
}
pub fn field_identity(address: Address) -> Field {
    from_bytes(&identity(address)).unwrap()
}
pub fn field(value: u8) -> Field {
    Field::from(value)
}
pub fn field_bytes(value: Field) -> [u8; 32] {
    to_bytes(&value)
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
        viewing_pubkey: ViewingKey::from_bytes(&[seed; 32]).pubkey(),
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
