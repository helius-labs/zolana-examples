use spr_program::{
    circuit::{hash_bytes, nonzero_hash_chain},
    input, poseidon, require, require_eq, CircuitError, Field, DOMAIN_PRIVATE_TX_BLINDING_V1,
    DOMAIN_TRANSACT_OUTPUT_BLINDING_SEED_V1, DOMAIN_TRANSACT_OUTPUT_BLINDING_V1,
};

/// A UTXO hash preimage as the shielded pool commits it. The swap circuits
/// witness every UTXO of the SPP transaction they authorize; SPP proves
/// ownership, tree inclusion and value conservation of the same UTXO hashes.
#[input]
pub struct ProofInputUtxo {
    pub domain: Field,
    pub tree_id: Field,
    pub owner_hash: Field,
    pub asset: Field,
    pub amount: Field,
    pub blinding: Field,
    pub data_hash: Field,
    pub ring_data_hash: Field,
    pub ring_program_id: Field,
}

impl ProofInputUtxo {
    /// A spendable UTXO outside every ring. Amount and tree id are not
    /// range-checked: the transact proof checks both for every UTXO hash it
    /// folds into the same private transaction hash.
    pub(super) fn assert_default_ring(&self) {
        require_eq(self.domain, Field::from(spr_interface::UTXO_DOMAIN));
        require_eq(self.ring_data_hash, Field::zero());
        require_eq(self.ring_program_id, Field::zero());
    }

    /// A default-ring UTXO that carries no data, such as a payout or change.
    pub(super) fn assert_plain(&self) {
        self.assert_default_ring();
        require_eq(self.data_hash, Field::zero());
    }

    pub fn hash(&self) -> Result<Field, CircuitError> {
        let ring = poseidon(&[self.ring_data_hash, self.ring_program_id])?;
        let owner = poseidon(&[self.owner_hash, self.blinding])?;
        poseidon(&[
            self.domain,
            self.tree_id,
            self.asset,
            self.amount,
            self.data_hash,
            ring,
            owner,
        ])
    }
}

/// The order terms the order UTXO commits as its data hash. See
/// `swap_program.md`, Order Terms.
#[input]
pub struct OrderTerms {
    /// `hash_bytes` of the destination mint.
    pub destination_asset: Field,
    pub destination_amount: u64,
    /// The maker's owner hash: receives the destination on take and the
    /// source on cancel, and recomputes from the cancel signer.
    pub maker_owner_hash: Field,
    /// The maker's tagged X25519 viewing pubkey, the second half of
    /// `maker_address`.
    pub maker_viewing_pk: [u8; 33],
    /// Unix seconds; revealed at take and cancel and checked against the
    /// Clock by the program.
    pub expiry: u64,
    /// The designated taker; committed, not checked by any swap circuit.
    pub taker_pk_fe: Field,
}

impl OrderTerms {
    /// `destination_amount > 0`; the `u64` input is range-checked on entry.
    pub(super) fn check(&self) {
        require(self.destination_amount != 0);
    }

    /// `maker_address` as a field element:
    /// `Poseidon(maker_owner_hash, hash_bytes(maker_viewing_pk))`.
    pub fn maker_address(&self) -> Result<Field, CircuitError> {
        poseidon(&[self.maker_owner_hash, hash_bytes(&self.maker_viewing_pk)?])
    }

    /// `Poseidon(destination_asset, destination_amount, maker_address,
    /// expiry, taker_pk_fe)`, the order UTXO's data hash.
    pub fn data_hash(&self) -> Result<Field, CircuitError> {
        poseidon(&[
            self.destination_asset,
            Field::from(self.destination_amount),
            self.maker_address()?,
            Field::from(self.expiry),
            self.taker_pk_fe,
        ])
    }

    /// The order UTXO commits these terms, sits outside every ring and holds
    /// a nonzero amount. Returns its UTXO hash. The owner stays a free
    /// witness: SPP spends the order UTXO only with the order-authority PDA
    /// signature.
    pub(super) fn check_order_utxo(&self, order: &ProofInputUtxo) -> Result<Field, CircuitError> {
        order.assert_default_ring();
        require_eq(order.data_hash, self.data_hash()?);
        require(order.amount != Field::zero());
        order.hash()
    }
}

/// What a swap circuit proves: the hash of its public inputs and the private
/// transaction hash. The generated entry circuit exposes it as the proof's
/// one public input.
pub struct Commitment(pub(super) Field);

impl Commitment {
    pub fn public_hash(&self) -> &Field {
        &self.0
    }
}

/// The private transaction hash of a transaction that creates no address:
/// `Poseidon(chain(inputs), chain(outputs), 0, blinding)`, each chain folding
/// only its nonzero UTXO hashes in slot order.
pub(super) fn check_private_tx_hash(
    inputs: &[Field],
    outputs: &[Field],
    blinding: Field,
    expected: Field,
) -> Result<(), CircuitError> {
    require_eq(
        poseidon(&[
            nonzero_hash_chain(inputs)?,
            nonzero_hash_chain(outputs)?,
            Field::zero(),
            blinding,
        ])?,
        expected,
    );
    Ok(())
}

/// The private transaction blinding and each output blinding follow SPP's
/// derivation from `first_nullifier` and `blinding_seed`.
/// `output_blindings[i]` is the blinding of output slot `i`.
pub(super) fn check_transaction_blindings(
    first_nullifier: Field,
    blinding_seed: Field,
    private_tx_blinding: Field,
    output_blindings: &[Field],
) -> Result<(), CircuitError> {
    require_eq(
        poseidon(&[
            Field::from(DOMAIN_PRIVATE_TX_BLINDING_V1),
            first_nullifier,
            blinding_seed,
        ])?,
        private_tx_blinding,
    );
    let output_seed = poseidon(&[
        Field::from(DOMAIN_TRANSACT_OUTPUT_BLINDING_SEED_V1),
        first_nullifier,
        blinding_seed,
    ])?;
    for (slot, blinding) in (0u64..).zip(output_blindings) {
        require_eq(
            poseidon(&[
                Field::from(DOMAIN_TRANSACT_OUTPUT_BLINDING_V1),
                first_nullifier,
                output_seed,
                Field::from(slot),
            ])?,
            *blinding,
        );
    }
    Ok(())
}
