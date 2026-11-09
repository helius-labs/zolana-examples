use spr_program::{
    circuit::{
        CheckedTransaction, ConfidentialTransaction, DataUtxo, PublicInputs, TokenUtxos, Utxo,
        UtxoTrait,
    },
    input, require, require_eq, CircuitError, ClientInputs, Field, TxContext, ZkProgram,
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
    /// The escrow UTXO. The circuit does not constrain its owner: SPP spends
    /// it only with the escrow-authority PDA's signature.
    pub escrow: Utxo,
    /// The escrow UTXO's state.
    pub terms: EscrowTerms,
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
    /// 1. Spend the escrow UTXO with its terms.
    /// 2. Check the signer is the creator and reveal the unlock timestamp.
    /// 3. Pay the whole escrow to the creator.
    /// 4. Check the transaction: outputs `[source output]`.
    pub fn execute(&self) -> Result<CheckedTransaction, CircuitError> {
        let private = &self.private;
        let public = &self.public;

        // 1. Spend the escrow UTXO with its terms.
        let mut escrow = DataUtxo::new_mut(&private.escrow, &private.terms)?.close();
        require(escrow.amount()? != 0);

        // 2. Check the signer is the creator and reveal the unlock timestamp.
        require_eq(escrow.creator.key().identity()?, public.owner_pk_field);
        require_eq(public.unlock, escrow.unlock);

        // 3. Pay the whole escrow to the creator.
        let mut source_output = TokenUtxos::new_init(&escrow.creator, &escrow.asset());
        escrow.transfer_all(&mut source_output)?;

        // 4. Check the transaction: outputs `[source output]`.
        ConfidentialTransaction::new(&private.tx_context, public)
            .with(escrow)
            .with(source_output)
            .check()
    }
}
