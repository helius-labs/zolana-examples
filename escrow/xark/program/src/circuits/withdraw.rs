use spr_program::{
    circuit::{
        CheckedTransaction, ConfidentialTransaction, DataUtxo, PublicInputs, TokenUtxos, Utxo,
        UtxoTrait,
    },
    input, require, require_eq, CircuitError, ClientInputs, Field, ShieldedAddress, TxContext,
    ZkProgram,
};

use super::EscrowTerms;

#[input]
#[derive(ZkProgram)]
pub struct Withdraw {
    pub private: WithdrawPrivateInputs,
    pub public: WithdrawPublicInputs,
}

#[input]
#[derive(ClientInputs)]
pub struct WithdrawPrivateInputs {
    pub tx_context: TxContext,
    pub terms: EscrowTerms,
    /// The escrow UTXO. The circuit does not constrain its owner: SPP spends
    /// it only with the escrow-authority PDA's signature.
    pub escrow: Utxo,
    /// The creator's shielded address. Its owner preimage, the signer
    /// identity and the nullifier public key, recomputes `terms.owner_hash`;
    /// it receives the withdrawal.
    pub creator: ShieldedAddress,
}

#[input]
#[derive(PublicInputs)]
pub struct WithdrawPublicInputs {
    /// The committed unlock timestamp; the program checks `now > unlock`.
    pub unlock: u64,
    /// `hash_bytes` of the creator signer's tagged pubkey, which the program
    /// computes from the signer account.
    pub owner_pk_field: Field,
}

impl Withdraw {
    /// 1. Reveal the committed unlock timestamp.
    /// 2. Check the signer is the creator.
    /// 3. Spend the escrow UTXO, which commits the terms.
    /// 4. Pay the whole escrow to the creator.
    /// 5. Check the transaction: outputs `[source output]`.
    pub fn execute(&self) -> Result<CheckedTransaction, CircuitError> {
        let private = &self.private;
        let public = &self.public;

        // 1. Reveal the committed unlock timestamp.
        require_eq(public.unlock, private.terms.unlock);

        // 2. Check the signer is the creator.
        let creator = private.creator.owner();
        require_eq(creator.hash()?, private.terms.owner_hash);
        require_eq(creator.key().identity()?, public.owner_pk_field);

        // 3. Spend the escrow UTXO, which commits the terms.
        let mut escrow =
            DataUtxo::new_mut_with_hash(&private.escrow, &private.terms.data_hash()?)?.close();
        require(escrow.amount()? != 0);

        // 4. Pay the whole escrow to the creator.
        let mut source_output = TokenUtxos::new_init(creator, &escrow.asset());
        escrow.transfer_all(&mut source_output)?;

        // 5. Check the transaction: outputs `[source output]`.
        ConfidentialTransaction::new(&private.tx_context, public)
            .with(escrow)
            .with(source_output)
            .check()
    }
}
