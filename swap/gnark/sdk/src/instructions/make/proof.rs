use anyhow::{bail, Result};
use spr_client::ProofInputUtxo;
use spr_transaction::instructions::transact::{PrivateTxHash, SppProofInputs, SppProofOutputUtxo};
use swap_program::instructions::make::MakePublicInput;
use swap_prover::{MakeProofInputs, OrderTermsProofInput};

use crate::{err, state::OrderUtxo};

/// The parts of the SPP transact this make CPIs into that the make proof has to
/// reproduce byte-for-byte, or the two proofs bind different `private_tx_hash`
/// values and the instruction can never land.
pub struct SppTxHashes {
    /// The spent source, the transaction's first input, hashed under the tree
    /// it is spent from. The make circuit requires the maker to own it.
    pub source: ProofInputUtxo,
    /// `SppProofInputs::private_tx_blinding()`, the last `private_tx_hash`
    /// preimage element.
    pub private_tx_blinding: [u8; 32],
    /// Raw id of the tree the order and change outputs are appended to; it is
    /// the second element of every output's commitment.
    pub output_tree_id: u16,
}

impl SppTxHashes {
    pub fn new(spp_proof_inputs: &SppProofInputs) -> Result<Self> {
        let source_input = spp_proof_inputs
            .input_utxos
            .first()
            .ok_or_else(|| err("missing source input"))?;
        Ok(Self {
            source: ProofInputUtxo::try_from(source_input).map_err(err)?,
            private_tx_blinding: spp_proof_inputs.private_tx_blinding().map_err(err)?,
            output_tree_id: spp_proof_inputs.output_tree_id,
        })
    }
}

pub struct MakeProofInputParams {
    pub order_utxo: OrderUtxo,
    pub change: SppProofOutputUtxo,
    pub spp_tx_hashes: SppTxHashes,
}

impl MakeProofInputParams {
    pub fn to_proof_inputs(&self) -> Result<MakeProofInputs> {
        let terms = &self.order_utxo.terms;
        if self.change.owner_address != Some(terms.destination) {
            bail!("change owner does not match order destination");
        }
        if self.change.asset != self.order_utxo.source_mint {
            bail!("change asset does not match order source mint");
        }
        if self.change.data_hash.is_some()
            || self.change.ring_data_hash.is_some()
            || self.change.ring_program_id.is_some()
        {
            bail!("change output must not carry data or ring commitments");
        }
        let order = OrderTermsProofInput::try_from(terms)?;
        // The program computes this from the maker signer (make's payer); the
        // circuit binds the terms' maker owner hash to it.
        let maker_owner_pk_field = terms
            .destination
            .signing_pubkey
            .owner_proof_input_hash()
            .map_err(err)?;
        // Both are created by this transaction, so both commit under the output
        // tree's id.
        let output_tree_id = self.spp_tx_hashes.output_tree_id;
        let order_utxo = ProofInputUtxo::try_from((
            &self
                .order_utxo
                .output_utxo(self.order_utxo.terms.destination.viewing_pubkey)?,
            output_tree_id,
        ))
        .map_err(err)?;
        // The program derives this from the maker signer; make's payer must
        // be the maker (`OrderUtxo::maker`).
        let order_owner_hash = self.order_utxo.owner_hash()?;
        if order_utxo.owner_hash != order_owner_hash {
            bail!("order utxo owner is not the maker's order-authority PDA");
        }
        // The maker funds the order from its own balance and signs the spend.
        // A utxo of the maker's order-authority PDA, such as an open order, is
        // not the maker's.
        let source = &self.spp_tx_hashes.source;
        if source.owner_hash != order.maker_owner_hash {
            bail!("source is not owned by the maker");
        }
        if source.asset != order_utxo.asset {
            bail!("source asset does not match order source mint");
        }
        if source.data_hash != [0u8; 32]
            || source.ring_data_hash != [0u8; 32]
            || source.ring_program_id != [0u8; 32]
        {
            bail!("source must not carry data or ring commitments");
        }
        let change = ProofInputUtxo::try_from((&self.change, output_tree_id)).map_err(err)?;
        let private_tx_hash = PrivateTxHash::new(
            &[source.hash().map_err(err)?, [0u8; 32]],
            &[change.hash().map_err(err)?, order_utxo.hash().map_err(err)?],
            &self.spp_tx_hashes.private_tx_blinding,
        )
        .hash()
        .map_err(err)?;
        let public_input_hash = MakePublicInput {
            private_tx_hash: &private_tx_hash,
            order_owner_hash: &order_owner_hash,
            maker_owner_pk_field: &maker_owner_pk_field,
        }
        .hash()
        .map_err(err)?;
        Ok(MakeProofInputs {
            public_input_hash,
            private_tx_hash,
            order_owner_hash,
            maker_owner_pk_field,
            maker_nullifier_pk: terms.destination.nullifier_pubkey,
            order,
            source: source.clone(),
            order_utxo,
            change,
            private_tx_blinding: self.spp_tx_hashes.private_tx_blinding,
        })
    }
}

#[cfg(test)]
mod tests {
    use solana_keypair::Keypair;
    use spr_keypair::shielded::ShieldedKeypair;
    use spr_transaction::{
        utxo::{SppProofInputUtxo, Utxo},
        Data, SOL_MINT,
    };

    use super::*;
    use crate::state::{OrderTerms, OrderUtxo};

    // TODO(tree-id): resolve the tree id from the tree account.
    const OUTPUT_TREE_ID: u16 = 0;
    const SOURCE_AMOUNT: u64 = 400_000;

    fn keypair(seed: u8) -> ShieldedKeypair {
        ShieldedKeypair::from_keypair(&Keypair::new_from_array([seed; 32])).expect("keypair")
    }

    fn order_utxo(maker: &ShieldedKeypair) -> OrderUtxo {
        OrderUtxo {
            terms: OrderTerms {
                destination_mint: SOL_MINT,
                destination_amount: 250_000,
                destination: maker.shielded_address().expect("maker address"),
                taker: None,
                expiry: 1_700_000_000,
            },
            blinding: crate::shared::test_blinding(11),
            source_mint: spr_transaction::Mint::SOL,
            source_amount: SOURCE_AMOUNT,
            destination_asset_id: 1,
        }
    }

    /// A plain SOL UTXO of `owner` worth `amount`.
    fn wallet_source(owner: &ShieldedKeypair, amount: u64) -> SppProofInputUtxo {
        crate::shared::test_input_utxo(
            Utxo {
                owner: owner.signing_pubkey(),
                asset: spr_transaction::Mint::SOL,
                amount,
                blinding: crate::shared::test_blinding(9),
                ring_program_id: None,
                data: Data::default(),
            },
            &owner.nullifier_key,
            OUTPUT_TREE_ID,
        )
    }

    /// The params of a make that spends `source` into `order_utxo` and a
    /// change of `change_amount` to the maker.
    fn params(
        order_utxo: OrderUtxo,
        source: SppProofInputUtxo,
        change_amount: u64,
    ) -> MakeProofInputParams {
        let change = SppProofOutputUtxo::new(
            spr_transaction::Mint::SOL,
            change_amount,
            order_utxo.terms.destination,
        )
        .expect("change output");
        MakeProofInputParams {
            spp_tx_hashes: SppTxHashes {
                source: ProofInputUtxo::try_from(&source).expect("source witness"),
                private_tx_blinding: [5u8; 32],
                output_tree_id: OUTPUT_TREE_ID,
            },
            order_utxo,
            change,
        }
    }

    // A make funded by an input whose value equals the order amount produces a
    // zero-value change output. That output is non-dummy (owner = order
    // destination), so SPP folds its real utxo hash into private_tx_hash. The
    // make proof must fold the same real hash -- never a zeroed hash keyed on
    // amount == 0 -- or the proof verifies against a different private_tx_hash
    // than the SPP transact it CPIs into and the instruction can never land.
    #[test]
    fn zero_change_folds_real_hash_matching_spp() {
        let maker = keypair(21);
        let order_utxo = order_utxo(&maker);
        let order_utxo_hash = order_utxo
            .to_input_utxo(OUTPUT_TREE_ID, 0)
            .expect("order input")
            .hash();
        let source = wallet_source(&maker, SOURCE_AMOUNT);
        let source_input_hash = source.hash();
        let params = params(order_utxo, source, 0);
        let change_hash = params.change.hash(OUTPUT_TREE_ID).expect("change hash");

        let inputs = params.to_proof_inputs().expect("proof inputs");

        // SPP (SppProofInputs::message_hash) hashes a non-dummy output at its
        // real hash; the make proof's public input must equal that.
        let expected = PrivateTxHash::new(
            &[source_input_hash, [0u8; 32]],
            &[change_hash, order_utxo_hash],
            &[5u8; 32],
        )
        .hash()
        .expect("private tx hash");
        assert_eq!(inputs.private_tx_hash, expected);
        assert_eq!(
            inputs.source.hash().expect("source witness hash"),
            source_input_hash
        );
    }

    // The make proof's owner public input and the order UTXO's owner must both
    // be the owner hash the program derives on chain for the maker's PDA
    // (`OrderAuthority::owner_hash`, i.e. `PdaOwner`).
    #[test]
    fn order_owner_hash_matches_program_pda_owner() {
        let maker = keypair(21);
        let maker_pubkey = maker
            .shielded_address()
            .expect("maker address")
            .solana_address()
            .expect("maker pubkey");
        let inputs = params(
            order_utxo(&maker),
            wallet_source(&maker, SOURCE_AMOUNT + 1),
            1,
        )
        .to_proof_inputs()
        .expect("proof inputs");

        let expected =
            *spr_program::program::PdaOwner::new(&crate::order_authority_pda(&maker_pubkey))
                .expect("pda owner")
                .owner_hash();
        assert_eq!(
            (inputs.order_owner_hash, inputs.order_utxo.owner_hash),
            (expected, expected)
        );
        // The maker identity the program computes from the signer, and the
        // preimage of the terms' maker owner hash, which owns the source.
        let maker_owner_pk_field =
            spr_hasher::primitives::solana_owner_identity(maker_pubkey.as_array())
                .expect("maker identity");
        assert_eq!(inputs.maker_owner_pk_field, maker_owner_pk_field);
        let maker_owner_hash =
            spr_keypair::hash::poseidon(&[&maker_owner_pk_field, &inputs.maker_nullifier_pk])
                .expect("maker owner hash");
        assert_eq!(
            (inputs.order.maker_owner_hash, inputs.source.owner_hash),
            (maker_owner_hash, maker_owner_hash)
        );
    }

    // The maker's open order UTXO is owned by its order-authority PDA, and the
    // maker knows its opening; another user's UTXO is not the maker's to sign
    // for. Neither may fund a make.
    #[test]
    fn rejects_source_not_owned_by_maker() {
        let maker = keypair(21);
        let open_order = order_utxo(&maker)
            .to_input_utxo(OUTPUT_TREE_ID, 0)
            .expect("open order input");
        let other_user = wallet_source(&keypair(22), SOURCE_AMOUNT);
        for (name, source) in [("open order", open_order), ("other user", other_user)] {
            let error = params(order_utxo(&maker), source, 0)
                .to_proof_inputs()
                .expect_err(name);
            assert_eq!(
                error.to_string(),
                "source is not owned by the maker",
                "{name}"
            );
        }
    }
}
