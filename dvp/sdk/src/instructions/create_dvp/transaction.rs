use anyhow::{ensure, Result};
use dvp_program::DVP_ADDRESS_TREE_ID;
use num_bigint::BigUint;
use solana_address::Address;
use zolana_client::{
    prover::field::be, CacheReadInputs, NonInclusionProof, ProofCompressed, ProofInputUtxo,
    ProverExt, PublicInputs, PublicTransfers, TransferInput, TransferInputs, TransferOutput,
    TreeSlotFields, STATE_TREE_HEIGHT,
};
use zolana_hasher::primitives::solana_owner_identity;
use zolana_interface::{
    instruction::{
        instruction_data::transact::{
            CircuitId, InputUtxo, OwnerTag, TransactIxData, TransactOutput, TransactProof,
            TreeContext, NO_UTXO_ROOT,
        },
        tag::TRANSACT,
    },
    state::cache::empty_cached_input_fields,
    tree_slot::{pack_input_flags, TreeSlot},
    INPUT_TREES, N_PUBLIC_SLOTS,
};
use zolana_keypair::{random_salt, ShieldedAddress, ViewingKey};
use zolana_program::{compression::ZERO_NULLIFIER_PUBKEY, TransactExternalData};
use zolana_transaction::SppProofOutputUtxo;

use super::{CreateDvp, CreateDvpDerivation};
use crate::{
    encryption::{encrypt_terms_messages, TERMS_RECIPIENTS},
    err,
    prover::DvpProverClient,
    state::SwapDvp,
};

/// Everything the create transaction needs besides the prover.
pub struct CreateDvpTransactionParams {
    pub terms: SwapDvp,
    /// Pays the transaction and is the SPP payer, the first signer the SPP
    /// proof binds.
    pub payer: Address,
    /// The seller, the buyer and the settlement authority, in that order:
    /// their viewing keys receive the terms messages.
    pub recipients: [ShieldedAddress; TERMS_RECIPIENTS],
    /// Raw id of the SPP tree: the DvP UTXO's output tree and the address
    /// slot's input tree. It must be `DVP_ADDRESS_TREE_ID` (D6).
    pub tree_id: u16,
    /// Non-inclusion of the DvP address ([`CreateDvpDerivation::address`]) in
    /// the tree's nullifier tree.
    pub non_inclusion: NonInclusionProof,
}

/// The create transaction before its proofs: the SPP prover witness and the
/// SPP instruction data, whose proof is zero until [`Self::prove`].
pub struct CreateDvpTransaction {
    payer: Address,
    creator: Address,
    pub derivation: CreateDvpDerivation,
    transfer_inputs: TransferInputs,
    pub spp: TransactIxData,
}

impl CreateDvpTransactionParams {
    /// Builds the create `transact` at SPP shape 1x2 (D1: there is no 1x1
    /// shape, so output slot 1 is compact padding, as in escrow `withdraw`):
    ///
    /// - input 0: the address slot, owned by `dvp_authority` with seed
    ///   `dvp_id` in tree `DVP_ADDRESS_TREE_ID`; its nullifier is the address,
    ///   so a `(creator, dvp_id)` pair is created once. Its tree is declared
    ///   with no UTXO root (`NO_UTXO_ROOT`) and the proof's nullifier root;
    /// - output 0: the DvP UTXO, tagged with the PDA, with no ciphertext (D9);
    /// - output 1: compact padding, left out of the instruction. SPP still
    ///   checks its derived blinding;
    /// - messages 0..3: the terms encrypted to the recipients (D7) under a
    ///   fresh transaction viewing key and a random salt.
    ///
    /// The public signer vector is `[payer, dvp_authority]`: the PDA is the
    /// only owner signer, which the program signs for (D8).
    pub fn build(self) -> Result<CreateDvpTransaction> {
        let Self {
            terms,
            payer,
            recipients,
            tree_id,
            non_inclusion,
        } = self;
        ensure!(
            tree_id == DVP_ADDRESS_TREE_ID,
            "the address slot is pinned to tree {DVP_ADDRESS_TREE_ID} (D6), not tree {tree_id}"
        );
        let derivation = CreateDvpDerivation::new(&terms, tree_id)?;
        ensure!(
            non_inclusion.leaf == derivation.address,
            "the non-inclusion proof is not for the DvP address"
        );
        let pda = derivation.dvp_authority.to_bytes();
        let pda_id = solana_owner_identity(&pda).map_err(err)?;
        let zero = [0u8; 32];

        let tx_viewing_key = ViewingKey::new();
        let salt = random_salt();
        let spp = TransactIxData {
            expiry_unix_ts: u64::MAX,
            tx_viewing_pk: *tx_viewing_key.pubkey().as_bytes(),
            salt,
            interface_transfers: Vec::new(),
            data_hash: None,
            ring_data_hash: None,
            outputs: vec![TransactOutput {
                utxo_hash: derivation.dvp_utxo_hash,
                owner_tag: OwnerTag::Inline(pda),
                data: None,
            }],
            messages: encrypt_terms_messages(&terms, &recipients, &tx_viewing_key, salt)?,
            private_tx_hash: derivation.private_tx_hash,
            circuit: CircuitId::ConfidentialEddsa(1, 2, N_PUBLIC_SLOTS as u8),
            proof: TransactProof::zeroed(),
            inputs: vec![InputUtxo {
                nullifier_hash: derivation.address,
                tree_index: 0,
            }],
            tree_contexts: vec![TreeContext {
                utxo_tree_root_index: NO_UTXO_ROOT,
                nullifier_tree_root_index: non_inclusion.root_index,
            }],
        };
        let external_data_hash = TransactExternalData::from(&spp)
            .hash(TRANSACT, &[], &[pda])
            .map_err(err)?;

        // Only tree slot 0 is used, with no UTXO root: the address slot is
        // not in the state tree.
        let mut tree_slots = [TreeSlot::ZERO; INPUT_TREES];
        if let Some(slot) = tree_slots.first_mut() {
            *slot = TreeSlot::new(tree_id, zero, non_inclusion.root);
        }
        let signer_pk_hashes = [
            solana_owner_identity(payer.as_array()).map_err(err)?,
            pda_id,
        ];
        // A compact output publishes owner 0.
        let output_owner_pk_hashes = [pda_id, zero];
        // The address slot takes the dummy bit, in tree slot 0.
        let input_flags = pack_input_flags(true, [0u8]).map_err(err)?;
        let cached_inputs = empty_cached_input_fields(1).map_err(err)?;
        let public_transfers = PublicTransfers::default();
        let public_input_hash = PublicInputs {
            nullifiers: &[derivation.address],
            output_hashes: &[derivation.dvp_utxo_hash, zero],
            tree_slots: &tree_slots,
            output_tree_id: tree_id,
            private_tx: &derivation.private_tx_hash,
            external_data_hash: &external_data_hash,
            public_transfers: &public_transfers,
            ring_program_id: &zero,
            input_flags: &input_flags,
            signer_pk_hashes: &signer_pk_hashes,
            output_owner_pk_hashes: Some(&output_owner_pk_hashes),
            cached_inputs,
        }
        .hash()
        .map_err(err)?;

        // The address slot nullifies under the public zero secret, so it is
        // complete as built.
        let address_input = TransferInput {
            utxo: derivation.address_slot.clone(),
            is_dummy: BigUint::ZERO,
            state_path_elements: vec![BigUint::ZERO; STATE_TREE_HEIGHT],
            state_path_index: BigUint::ZERO,
            nullifier_low_value: be(&non_inclusion.low_element),
            nullifier_next_value: be(&non_inclusion.high_element),
            nullifier_low_path_elements: non_inclusion.path.iter().map(be).collect(),
            nullifier_low_path_index: BigUint::from(non_inclusion.low_element_index),
            tree_slot: BigUint::ZERO,
            nullifier: be(&derivation.address),
            owner_pk_hash: be(&pda_id),
            nullifier_secret: Some(BigUint::ZERO),
        };
        let dvp_output = TransferOutput {
            utxo: derivation.dvp_utxo.clone(),
            is_dummy: BigUint::ZERO,
            hash: be(&derivation.dvp_utxo_hash),
            owner_pk_hash: be(&pda_id),
            nullifier_pk: be(&ZERO_NULLIFIER_PUBKEY),
        };
        let padding = SppProofOutputUtxo {
            blinding: derivation.padding_blinding,
            compact: true,
            ..SppProofOutputUtxo::default()
        };
        let padding_output = TransferOutput {
            utxo: ProofInputUtxo::try_from((&padding, tree_id)).map_err(err)?,
            is_dummy: BigUint::from(1u8),
            hash: BigUint::ZERO,
            owner_pk_hash: BigUint::ZERO,
            nullifier_pk: BigUint::ZERO,
        };
        let transfer_inputs = TransferInputs {
            inputs: vec![address_input],
            outputs: vec![dvp_output, padding_output],
            tree_slots: TreeSlotFields::encode_all(&tree_slots),
            output_tree_id: BigUint::from(tree_id),
            blinding_seed: be(&derivation.blinding_seed),
            external_data_hash: be(&external_data_hash),
            private_tx_hash: be(&derivation.private_tx_hash),
            public_assets: core::array::from_fn(|_| BigUint::ZERO),
            public_amounts: core::array::from_fn(|_| BigUint::ZERO),
            ring_program_id: BigUint::ZERO,
            signer_pk_hashes: signer_pk_hashes.iter().map(be).collect(),
            input_flags: be(&input_flags),
            cache: CacheReadInputs::uncached(cached_inputs),
            published_output_owner_pk_hashes: output_owner_pk_hashes.iter().map(be).collect(),
            public_input_hash: be(&public_input_hash),
        };
        Ok(CreateDvpTransaction {
            payer,
            creator: terms.creator,
            derivation,
            transfer_inputs,
            spp,
        })
    }
}

impl CreateDvpTransaction {
    /// Proves the SPP transaction with `spp_prover` and the create proof
    /// locally, and returns the instruction builder for `tree`, the account
    /// of the tree with id `tree_id`.
    pub fn prove(self, spp_prover: &impl ProverExt, tree: Address) -> Result<CreateDvp> {
        let Self {
            payer,
            creator,
            derivation,
            transfer_inputs,
            mut spp,
        } = self;
        let proof = spp_prover.prove_transfer(&transfer_inputs).map_err(err)?;
        spp.proof = ProofCompressed::try_from(proof)
            .map_err(err)?
            .to_transact_proof();
        let create_proof = DvpProverClient::new()
            .prove_create_dvp(&derivation.to_proof_inputs()?)?
            .into();
        Ok(CreateDvp {
            payer,
            creator,
            tree,
            dvp_id: derivation.dvp_id,
            bump: derivation.bump,
            create_proof,
            spp,
        })
    }
}

#[cfg(test)]
mod tests {

    use super::*;
    use crate::{
        encryption::decrypt_terms_message,
        index::fixture::{create_params, fixture_parties, keypair, sample_terms},
    };

    #[test]
    fn build_publishes_the_dvp_utxo_and_three_terms_messages() {
        let parties = fixture_parties();
        let terms = sample_terms(&parties);
        let transaction = create_params(&terms, &parties).build().expect("build");
        let derivation = &transaction.derivation;
        let spp = &transaction.spp;
        assert_eq!(
            (
                spp.circuit,
                spp.outputs.clone(),
                spp.inputs.clone(),
                spp.private_tx_hash,
                spp.messages.len()
            ),
            (
                CircuitId::ConfidentialEddsa(1, 2, N_PUBLIC_SLOTS as u8),
                vec![TransactOutput {
                    utxo_hash: derivation.dvp_utxo_hash,
                    owner_tag: OwnerTag::Inline(derivation.dvp_authority.to_bytes()),
                    data: None,
                }],
                vec![InputUtxo {
                    nullifier_hash: derivation.address,
                    tree_index: 0,
                }],
                derivation.private_tx_hash,
                TERMS_RECIPIENTS
            )
        );
        let tx_viewing_pk =
            zolana_keypair::P256Pubkey::from_bytes(spp.tx_viewing_pk).expect("tx viewing pk");
        for ((index, message), party) in (0u32..).zip(&spp.messages).zip(&parties) {
            let decrypted = decrypt_terms_message(
                &party.viewing_key,
                &message.data,
                &tx_viewing_pk,
                spp.salt,
                index,
            )
            .expect("decrypt");
            assert_eq!(
                (message.view_tag, decrypted),
                (
                    party
                        .signing_pubkey()
                        .confidential_view_tag()
                        .expect("view tag"),
                    terms.clone()
                )
            );
        }
    }

    #[test]
    fn build_rejects_another_address_tree() {
        let parties = fixture_parties();
        let terms = sample_terms(&parties);
        let params = CreateDvpTransactionParams {
            tree_id: DVP_ADDRESS_TREE_ID + 1,
            ..create_params(&terms, &parties)
        };
        assert!(params.build().is_err());
    }

    #[test]
    fn build_rejects_a_non_inclusion_proof_for_another_leaf() {
        let parties = fixture_parties();
        let terms = sample_terms(&parties);
        let mut params = create_params(&terms, &parties);
        params.non_inclusion.leaf = [1; 32];
        assert!(params.build().is_err());
    }

    #[test]
    fn build_rejects_a_recipient_the_terms_do_not_name() {
        let parties = fixture_parties();
        let terms = sample_terms(&parties);
        let mut params = create_params(&terms, &parties);
        let outsider = keypair(99);
        let [_, buyer, authority] = &parties;
        params.recipients = [
            outsider.shielded_address().expect("outsider address"),
            buyer.shielded_address().expect("buyer address"),
            authority.shielded_address().expect("authority address"),
        ];
        assert!(params.build().is_err());
    }
}
