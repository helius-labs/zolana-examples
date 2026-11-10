use solana_address::Address;
use solana_keypair::Keypair;
use spr_keypair::{ShieldedAddress, ShieldedKeypair};
use spr_program::{
    circuit::CheckedTransaction, to_bytes, ClientInputs, Field, InputRecords, ProgramProofInputs,
    ProgramTransaction, TxContext,
};
use spr_transaction::{instructions::transact::FinalizedTransaction, Data, Mint, Utxo, WalletUtxo};
use std::ops::{Deref, DerefMut};
use swap_program::circuits as circuit;
use swap_sdk::{
    instructions::take::take_blinding_seed,
    order_authority,
    proof::CircuitId,
    shared::identity,
    state::{OrderTerms, OrderUtxo},
};

/// The tree the spent UTXOs are in. Non-zero, and different from the output
/// tree, so a swapped or dropped tree id changes the commitments the proof
/// binds.
pub const INPUT_TREE_ID: u16 = 3;
pub const OUTPUT_TREE_ID: u16 = 7;
pub const EXPIRY: u64 = 1_700_000_000;
pub const SOURCE_AMOUNT: u64 = 1_000;
pub const DESTINATION_AMOUNT: u64 = 250;
pub const ORDER_LEAF_INDEX: u64 = 5;

pub trait Execute: spr_program::xark::Input {
    fn execute(&self) -> Result<CheckedTransaction, spr_program::CircuitError>;
}

macro_rules! program {
    ($($name:ident),*) => {$(
        impl Execute for circuit::$name {
            fn execute(&self) -> Result<CheckedTransaction, spr_program::CircuitError> {
                circuit::$name::execute(self)
            }
        }
        pub type $name = Fixture<circuit::$name>;
    )*};
}
program!(Make, Take, Cancel);

/// A program's inputs and the native records they were converted from.
pub struct Fixture<P> {
    pub program: P,
    records: InputRecords,
    /// The transaction's sender: the maker for make and cancel, the taker
    /// for take.
    pub sender: ShieldedKeypair,
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
    pub fn sender_address(&self) -> ShieldedAddress {
        self.sender.shielded_address().unwrap()
    }

    pub fn create_program_transaction(&self) -> anyhow::Result<ProgramTransaction> {
        Ok(self.records.create_program_transaction(
            &self.program,
            self.execute()?,
            &self.sender_address(),
            Address::default(),
        )?)
    }

    pub fn create_finalized_transaction(&self) -> anyhow::Result<FinalizedTransaction> {
        Ok(self.create_program_transaction()?.finalized)
    }

    /// Converts `address` as a circuit input and records its viewing key.
    pub fn address(&mut self, address: ShieldedAddress) -> spr_program::ShieldedAddress {
        self.records.address(&address).unwrap()
    }

    /// Converts `mint` as a circuit input and records it.
    pub fn asset(&mut self, mint: Mint) -> [u8; 32] {
        self.records.asset(mint).unwrap()
    }

    /// Converts `terms` as a circuit input and records their mint.
    pub fn terms(&mut self, terms: &OrderTerms) -> circuit::OrderTerms {
        circuit::OrderTerms::from_client(terms.circuit_terms().unwrap(), &mut self.records).unwrap()
    }
}

pub fn keypair() -> ShieldedKeypair {
    ShieldedKeypair::from_keypair(&Keypair::new()).unwrap()
}

pub fn source_mint() -> Mint {
    Mint::new(Address::new_from_array([3; 32]), 2)
}

pub fn destination_mint() -> Mint {
    Mint::new(Address::new_from_array([4; 32]), 3)
}

pub fn fe(value: u8) -> [u8; 32] {
    let mut bytes = [0; 32];
    bytes[31] = value;
    bytes
}

pub fn field(value: u8) -> Field {
    Field::from(value)
}

pub fn field_bytes(value: Field) -> [u8; 32] {
    to_bytes(&value)
}

pub fn context(seed: &[u8; 32]) -> TxContext {
    InputRecords::context(seed, Some(OUTPUT_TREE_ID)).unwrap()
}

/// The owner identity of a keypair's Solana address.
pub fn identity_of(keypair: &ShieldedKeypair) -> [u8; 32] {
    identity(
        &keypair
            .shielded_address()
            .unwrap()
            .solana_address()
            .unwrap(),
    )
    .unwrap()
}

pub fn terms(maker: &ShieldedKeypair, taker: Option<[u8; 32]>) -> OrderTerms {
    OrderTerms {
        destination_mint: destination_mint(),
        destination_amount: DESTINATION_AMOUNT,
        maker: maker.shielded_address().unwrap(),
        expiry: EXPIRY,
        taker,
    }
}

pub fn order_utxo(terms: OrderTerms) -> OrderUtxo {
    OrderUtxo {
        terms,
        blinding: fe(7),
        source_mint: source_mint(),
        source_amount: SOURCE_AMOUNT,
    }
}

/// `amount` of `mint` owned by `owner`, at leaf 6 of [`INPUT_TREE_ID`].
pub fn wallet_utxo(owner: &ShieldedKeypair, mint: Mint, amount: u64) -> WalletUtxo {
    spr_test_utils::utxo::wallet(
        Utxo {
            owner: owner.signing_pubkey(),
            asset: mint,
            amount,
            blinding: fe(13),
            ring_program_id: None,
            data: Data::default(),
        },
        &owner.nullifier_key,
        INPUT_TREE_ID,
        6,
        None,
        None,
    )
    .unwrap()
}

/// The order UTXO address a make by `maker` encrypted to `taker` uses: the
/// maker's order-authority PDA with the taker's viewing key.
pub fn order_address(maker: &ShieldedKeypair, taker: &ShieldedKeypair) -> ShieldedAddress {
    order_authority(&maker.shielded_address().unwrap().solana_address().unwrap())
        .address(taker.viewing_pubkey())
        .unwrap()
}

/// A make locking `source_amount` of a `source_amount + change_amount` source
/// owned by the maker, designated to and encrypted for `taker`.
pub fn make_inputs(source_amount: u64, change_amount: u64) -> Make {
    let maker = keypair();
    let taker = keypair();
    let order_address = order_address(&maker, &taker);
    let source = wallet_utxo(&maker, source_mint(), source_amount + change_amount);
    let maker_owner_pk_field = spr_program::from_bytes(&identity_of(&maker)).unwrap();
    let mut fixture = Fixture {
        program: circuit::Make {
            private: circuit::MakePrivateInputs {
                tx_context: context(&fe(41)),
                source: spr_program::circuit::Utxo::dummy(),
                source_amount,
                destination_mint: [0; 32],
                destination_amount: DESTINATION_AMOUNT,
                expiry: EXPIRY,
                taker_pk_fe: identity_of(&taker),
            },
            public: circuit::MakePublicInputs {
                maker_owner_pk_field,
                order_owner: spr_program::ShieldedAddress::default(),
            },
        },
        records: InputRecords::new(),
        sender: maker,
    };
    fixture.program.private.destination_mint = fixture.asset(destination_mint());
    fixture.program.private.source = fixture.records.utxo(&source).unwrap();
    fixture.program.public.order_owner = fixture.address(order_address);
    fixture
}

/// The order terms a make of `inputs` commits: its terms with the source
/// owner, the maker, as the maker.
pub fn make_terms(inputs: &mut Make) -> circuit::OrderTerms {
    let private = &inputs.program.private;
    let (destination_mint, destination_amount, expiry, taker_pk_fe) = (
        private.destination_mint,
        private.destination_amount,
        private.expiry,
        private.taker_pk_fe,
    );
    let maker = *inputs.address(inputs.sender_address()).owner();
    circuit::OrderTerms {
        destination_mint,
        destination_amount,
        maker,
        expiry,
        taker_pk_fe,
    }
}

/// A take of the order of [`order_utxo`] at [`ORDER_LEAF_INDEX`] by `taker`,
/// with the order's `taker` identity and the take blinding seed.
pub fn take_inputs(order: &OrderUtxo, taker: ShieldedKeypair) -> Take {
    let order_input = order
        .to_input_utxo(INPUT_TREE_ID, ORDER_LEAF_INDEX)
        .unwrap();
    let taker_input = wallet_utxo(&taker, destination_mint(), DESTINATION_AMOUNT);
    let mut fixture = Fixture {
        program: circuit::Take {
            private: circuit::TakePrivateInputs {
                tx_context: context(&take_blinding_seed(&order.blinding).unwrap()),
                terms: circuit::OrderTerms::default(),
                order_utxo: spr_program::circuit::Utxo::dummy(),
                taker_in: spr_program::circuit::Utxo::dummy(),
                maker_address: spr_program::ShieldedAddress::default(),
            },
            public: circuit::TakePublicInputs {
                expiry: order.terms.expiry,
                first_nullifier: spr_program::from_bytes(&order_input.nullifier).unwrap(),
            },
        },
        records: InputRecords::new(),
        sender: taker,
    };
    fixture.program.private.terms = fixture.terms(&order.terms);
    fixture.program.private.order_utxo = fixture.records.utxo(&order_input).unwrap();
    fixture.program.private.taker_in = fixture.records.utxo(&taker_input).unwrap();
    fixture.program.private.maker_address = fixture.address(order.terms.maker);
    fixture
}

/// A cancel of an order of `maker` at [`ORDER_LEAF_INDEX`].
pub fn cancel_inputs() -> Cancel {
    let maker = keypair();
    let order = order_utxo(terms(&maker, None));
    let order_input = order
        .to_input_utxo(INPUT_TREE_ID, ORDER_LEAF_INDEX)
        .unwrap();
    let maker_owner_pk_field = spr_program::from_bytes(&identity_of(&maker)).unwrap();
    let mut fixture = Fixture {
        program: circuit::Cancel {
            private: circuit::CancelPrivateInputs {
                tx_context: context(&fe(43)),
                terms: circuit::OrderTerms::default(),
                order_utxo: spr_program::circuit::Utxo::dummy(),
            },
            public: circuit::CancelPublicInputs {
                expiry: EXPIRY,
                maker_owner_pk_field,
            },
        },
        records: InputRecords::new(),
        sender: maker,
    };
    fixture.program.private.terms = fixture.terms(&order.terms);
    fixture.program.private.order_utxo = fixture.records.utxo(&order_input).unwrap();
    fixture
}

/// Proves `inputs` with the checked circuit, verifies the proof against `vk`
/// with the transaction's public hash, and checks a tampered public hash
/// fails. Returns the program transaction and the proof.
pub fn prove_and_verify<P: Execute>(
    id: CircuitId,
    inputs: &Fixture<P>,
    vk: &groth16_solana::groth16::Groth16Verifyingkey,
) -> (ProgramTransaction, spr_program::CompressedProof) {
    let tx = inputs.create_program_transaction().unwrap();
    let (_private, circuit) = swap_sdk::proof::checked_circuit(id).unwrap();
    circuit.check_constrained(tx.proof_inputs.clone()).unwrap();
    let proof = swap_sdk::proof::prove(id, &tx.proof_inputs).unwrap();
    assert!(
        proof.a.iter().any(|&byte| byte != 0),
        "proof_a must not be all zero"
    );
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
            .is_err(),
        "verification must fail for a tampered public input"
    );
    (tx, proof)
}

/// Checks that both the native execution and the compiled circuit refuse
/// `inputs`.
pub fn assert_invalid<P: Execute>(id: CircuitId, inputs: &Fixture<P>) {
    let native = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        spr_program::checked(|| inputs.execute())
    }));
    assert!(
        !matches!(native, Ok(Ok(_))),
        "the native circuit accepted invalid inputs"
    );
    let (_private, circuit) = swap_sdk::proof::checked_circuit(id).unwrap();
    let error = circuit
        .check(ProgramProofInputs::new(&inputs.program, Field::zero()))
        .expect_err("the compiled circuit must reject invalid inputs");
    assert!(
        error.to_string().contains("program.execute()"),
        "expected the program's constraint to fail before the outer public-hash check: {error}"
    );
}
