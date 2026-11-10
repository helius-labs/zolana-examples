use spr_program::{
    circuit::{
        CheckedTransaction, ConfidentialTransaction, DataUtxo, PublicInputs, TokenUtxos, Utxo,
        UtxoTrait,
    },
    input, require, require_eq, Asset, CircuitError, ClientInputs, Field, ShieldedAddress,
    TxContext, ZkProgram,
};

use super::OrderTerms;

#[input]
#[derive(ZkProgram)]
pub struct Make {
    pub private: MakePrivateInputs,
    pub public: MakePublicInputs,
}

#[input]
#[derive(ClientInputs)]
pub struct MakePrivateInputs {
    pub tx_context: TxContext,
    /// The maker's spent source UTXO. SPP authorizes the spend with the
    /// maker's signature; its owner becomes the order's maker and receives
    /// the change.
    pub source: Utxo,
    /// The amount locked in the order UTXO.
    pub source_amount: u64,
    /// The order terms other than the maker; see [`OrderTerms`].
    pub destination_mint: Asset,
    pub destination_amount: u64,
    pub expiry: u64,
    pub taker_pk_fe: [u8; 32],
}

#[input]
#[derive(PublicInputs)]
pub struct MakePublicInputs {
    /// `hash_bytes` of the maker signer's tagged pubkey, which the program
    /// computes from the signer account.
    pub maker_owner_pk_field: Field,
    /// The order UTXO's address: the maker's order-authority PDA with the
    /// taker's viewing key, so the taker can decrypt the order. The program
    /// derives the PDA from the maker signer and hashes the address. SPP
    /// accepts a data-bearing output owned by any owner signer, the maker
    /// included, so this binds the order UTXO to the PDA that only the
    /// program signs for.
    pub order_owner: ShieldedAddress,
}

impl Make {
    /// 1. Spend the source UTXO and check its owner is the maker signer.
    /// 2. Create the order UTXO, owned by the public `order_owner`, and lock
    ///    `source_amount` in it; the rest of the source returns to its owner
    ///    as change.
    /// 3. Set and check the order terms; the source owner is the maker.
    /// 4. Check the transaction: outputs `[change, order UTXO]`.
    pub fn execute(&self) -> Result<CheckedTransaction, CircuitError> {
        let private = &self.private;

        // 1. Spend the source UTXO of the maker signer.
        let mut tokens = TokenUtxos::new_mut(core::array::from_ref(&private.source))?;
        let maker = tokens.owner();
        require_eq(maker.key().identity()?, self.public.maker_owner_pk_field);

        // 2. Lock `source_amount` in the order UTXO.
        require(private.source_amount != 0);
        let mut order = DataUtxo::<OrderTerms>::new_init(self.public.order_owner.owner())
            .with_asset(&tokens.asset())?;
        tokens.transfer(&mut order, &private.source_amount)?;

        // 3. Set and check the order terms.
        order.destination_mint = private.destination_mint;
        order.destination_amount = private.destination_amount;
        order.maker = maker;
        order.expiry = private.expiry;
        order.taker_pk_fe = private.taker_pk_fe;
        order.assert_valid();

        // 4. Check the transaction: outputs `[change, order UTXO]`.
        ConfidentialTransaction::new(&private.tx_context, &self.public)
            .with(tokens)
            .with(order)
            .check()
    }
}
