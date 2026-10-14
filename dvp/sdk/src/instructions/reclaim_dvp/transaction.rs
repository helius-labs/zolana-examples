use anyhow::{bail, Result};
use dvp_prover::ReclaimDvpProofInputs;
use solana_address::Address;
use zolana_client::ZolanaIndexer;
use zolana_keypair::{random_blinding, Curve, ShieldedAddress, ShieldedKeypair};
use zolana_transaction::{instructions::transact::SppProofInputs, SppProofOutputUtxo};

use super::{ReclaimDvp, ReclaimDvpProofInputParams};
use crate::{
    encryption::encrypt_outputs,
    err,
    shared::{assign_output_blindings, ensure_output_tree, prove_pda_transact},
    state::Deposit,
};

/// Everything the reclaim transaction needs besides the signer's keys and
/// the prover.
pub struct ReclaimDvpTransactionParams {
    /// The reclaimed deposit; the signer must fund its leg (the seller for
    /// delivery, the buyer for payment).
    pub deposit: Deposit,
    /// Pays the transaction and is the SPP payer, the first signer the SPP
    /// proof binds.
    pub payer: Address,
    /// Where the refund goes; the signer's own shielded address when `None`.
    /// It needs an Ed25519 or PDA signing key.
    pub refund_destination: Option<ShieldedAddress>,
    /// Raw id of the tree the refund is appended to.
    pub tree_id: u16,
}

/// The reclaim transaction before its proofs: the SPP proof inputs and the
/// reclaim proof inputs over the same transaction.
pub struct ReclaimDvpTransaction {
    signer: Address,
    creator: Address,
    bump: u8,
    pub spp_proof_inputs: SppProofInputs,
    pub proof_inputs: ReclaimDvpProofInputs,
}

impl ReclaimDvpTransactionParams {
    /// Builds the reclaim `transact` at SPP shape 1x2 (there is no 1x1
    /// shape, so output slot 1 is compact padding, as in escrow `withdraw`):
    ///
    /// - input 0: the deposit, owned by `dvp_authority` under the zero
    ///   nullifier secret;
    /// - output 0: the refund, the deposit's asset and full amount with no
    ///   data, encrypted to the refund destination;
    /// - output 1: compact padding, left out of the instruction.
    ///
    /// The blinding seed is random (the reclaim circuit derives nothing from
    /// the terms); both output blindings are derived from it with the
    /// deposit's nullifier, as SPP requires. The transaction viewing key is
    /// derived from `signer`'s viewing key and that nullifier.
    ///
    /// Errors unless `signer` funds the deposit's leg, so the buyer cannot
    /// build a reclaim of the seller's delivery deposit, or the other way,
    /// and unless the deposit sits in the tree with id `tree_id`, which the
    /// instruction reads its input from.
    pub fn build(self, signer: &ShieldedKeypair) -> Result<ReclaimDvpTransaction> {
        let Self {
            deposit,
            payer,
            refund_destination,
            tree_id,
        } = self;
        ensure_output_tree("the deposit", deposit.tree_id, tree_id)?;
        let signer_address = signer.shielded_address().map_err(err)?;
        let refund_destination = refund_destination.unwrap_or(signer_address);
        if refund_destination.signing_pubkey.curve().map_err(err)? == Curve::P256 {
            bail!("refund destination has a P256 signing key");
        }

        let input = deposit.to_input_utxo()?;
        let first_nullifier = input.nullifier;
        let blinding_seed = random_blinding();
        let mut outputs = [
            SppProofOutputUtxo::new(deposit.mint, deposit.amount, refund_destination)
                .map_err(err)?,
            SppProofOutputUtxo {
                compact: true,
                ..SppProofOutputUtxo::default()
            },
        ];
        assign_output_blindings(&mut outputs, &first_nullifier, &blinding_seed)?;
        let [refund, _] = &outputs;
        let refund = refund.clone();

        let tx_viewing_key = signer
            .get_transaction_viewing_key(&first_nullifier)
            .map_err(err)?;
        let spp_proof_inputs = SppProofInputs {
            external_data: encrypt_outputs(&outputs, &tx_viewing_key, tree_id)?,
            input_utxos: vec![input],
            output_utxos: outputs.into(),
            payer,
            blinding_seed,
            output_tree_id: tree_id,
            cache_accounts: Default::default(),
        };

        let signer = signer_address.solana_address().map_err(err)?;
        let creator = deposit.terms.creator;
        let (_, bump) = deposit.terms.dvp_authority()?;
        let proof_inputs = ReclaimDvpProofInputParams {
            terms: deposit.terms.clone(),
            signer,
            deposit,
            refund,
            refund_tree_id: tree_id,
            private_tx_blinding: spp_proof_inputs.private_tx_blinding().map_err(err)?,
        }
        .to_proof_inputs()?;
        Ok(ReclaimDvpTransaction {
            signer,
            creator,
            bump,
            spp_proof_inputs,
            proof_inputs,
        })
    }
}

impl ReclaimDvpTransaction {
    /// Proves the SPP transaction through `indexer` (which serves the
    /// deposit's inclusion proof) and the reclaim proof locally, and returns
    /// the instruction builder for `tree`, the account of the tree with id
    /// `tree_id`.
    pub fn prove(self, indexer: &ZolanaIndexer, tree: Address) -> Result<ReclaimDvp> {
        let Self {
            signer,
            creator,
            bump,
            spp_proof_inputs,
            proof_inputs,
        } = self;
        let payer = spp_proof_inputs.payer;
        let spp = prove_pda_transact(indexer, spp_proof_inputs, &proof_inputs.private_tx_hash)?;
        let reclaim_proof = proof_inputs.prove().map_err(err)?.into();
        Ok(ReclaimDvp {
            payer,
            signer,
            tree,
            creator,
            dvp_id: proof_inputs.dvp_id,
            bump,
            reclaim_proof,
            spp,
        })
    }
}

#[cfg(test)]
mod tests {
    use zolana_keypair::{PublicKey, ViewingKey};

    use super::*;
    use crate::index::fixture::{deposit_fixture, shielded_transaction, spendable_amounts};

    /// Leaf the fixture reclaim appends its refund at.
    const REFUND_LEAF: u64 = 30;

    fn params(
        deposit: &Deposit,
        refund_destination: Option<ShieldedAddress>,
    ) -> ReclaimDvpTransactionParams {
        ReclaimDvpTransactionParams {
            deposit: deposit.clone(),
            payer: Address::new_from_array([7; 32]),
            refund_destination,
            tree_id: deposit.tree_id,
        }
    }

    #[test]
    fn build_refunds_the_whole_deposit_to_the_signer() {
        let fixture = deposit_fixture();
        let [seller, buyer] = &fixture.funders;
        let [delivery, _] = &fixture.deposits;
        let transaction = params(delivery, None).build(seller).expect("build");
        let inputs = &transaction.spp_proof_inputs;
        let tx = shielded_transaction(inputs, REFUND_LEAF);
        let mint = &delivery.mint.asset;
        let deposit_input = delivery.to_input_utxo().expect("deposit input");
        assert_eq!(
            (
                inputs
                    .input_utxos
                    .iter()
                    .map(|input| (input.utxo_hash, input.nullifier))
                    .collect::<Vec<_>>(),
                inputs
                    .output_utxos
                    .iter()
                    .map(|output| output.compact)
                    .collect::<Vec<_>>(),
                inputs.external_data.outputs.len(),
                transaction.proof_inputs.leg,
                spendable_amounts(seller, &tx, &fixture.registry, mint),
                spendable_amounts(buyer, &tx, &fixture.registry, mint),
            ),
            (
                vec![(deposit_input.utxo_hash, deposit_input.nullifier)],
                vec![false, true],
                1,
                false,
                vec![delivery.amount],
                Vec::new(),
            )
        );
    }

    #[test]
    fn build_refunds_to_a_chosen_destination() {
        let fixture = deposit_fixture();
        let [_, buyer] = &fixture.funders;
        let [_, payment] = &fixture.deposits;
        let destination = fixture.authority.shielded_address().expect("address");
        let transaction = params(payment, Some(destination))
            .build(buyer)
            .expect("build");
        let tx = shielded_transaction(&transaction.spp_proof_inputs, REFUND_LEAF);
        let mint = &payment.mint.asset;
        assert_eq!(
            (
                transaction.proof_inputs.leg,
                spendable_amounts(&fixture.authority, &tx, &fixture.registry, mint),
                spendable_amounts(buyer, &tx, &fixture.registry, mint),
            ),
            (true, vec![payment.amount], Vec::new())
        );
    }

    #[test]
    fn build_refuses_the_other_partys_deposit() {
        let fixture = deposit_fixture();
        let [seller, buyer] = &fixture.funders;
        let [delivery, payment] = &fixture.deposits;
        assert!(params(delivery, None).build(buyer).is_err());
        assert!(params(payment, None).build(seller).is_err());
        assert!(params(payment, None).build(&fixture.authority).is_err());
    }

    #[test]
    fn build_refuses_a_p256_refund_destination() {
        let fixture = deposit_fixture();
        let [seller, _] = &fixture.funders;
        let [delivery, _] = &fixture.deposits;
        let p256 = ShieldedAddress {
            signing_pubkey: PublicKey::from_p256(&ViewingKey::new().pubkey()),
            ..seller.shielded_address().expect("address")
        };
        assert!(params(delivery, Some(p256)).build(seller).is_err());
    }

    /// The instruction reads its input from the output tree.
    #[test]
    fn build_refuses_a_deposit_outside_the_output_tree() {
        let fixture = deposit_fixture();
        let [seller, _] = &fixture.funders;
        let [delivery, _] = &fixture.deposits;
        let error = ReclaimDvpTransactionParams {
            tree_id: delivery.tree_id + 1,
            ..params(delivery, None)
        }
        .build(seller)
        .err()
        .expect("a deposit outside the output tree");
        assert_eq!(
            error.to_string(),
            format!(
                "the deposit is in tree {}, not in the output tree {}",
                delivery.tree_id,
                delivery.tree_id + 1
            )
        );
    }
}
