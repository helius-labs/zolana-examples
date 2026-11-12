use anyhow::Result;
use solana_instruction::{AccountMeta, Instruction};
use solana_pubkey::Pubkey;
use spr_interface::instruction::instruction_data::transact::{MessageData, TransactIxData};
use spr_keypair::ShieldedAddress;
use spr_program::instruction::Transact;
use spr_transaction::TransactionError;

use crate::{err, order_authority_pda_and_bump, tag, MakeIxData, MakeProof, MarkerData};

pub struct OrderMarker {
    pub order_utxo_hash: [u8; 32],
    pub maker_pubkey: Pubkey,
    pub taker_address: ShieldedAddress,
}

impl OrderMarker {
    pub fn message(self) -> Result<MessageData, TransactionError> {
        Ok(MessageData {
            view_tag: self.taker_address.signing_pubkey.confidential_view_tag()?,
            data: borsh::to_vec(&MarkerData {
                order_utxo_hash: self.order_utxo_hash,
                maker_pubkey: self.maker_pubkey.to_bytes(),
            })
            .map_err(|e| TransactionError::Serialize(e.to_string()))?,
        })
    }
}

pub struct Make {
    /// The maker: fee payer, marker signer and owner of the spent source. Its
    /// order-authority PDA owns the order.
    pub payer: Pubkey,
    pub tree: Pubkey,
    pub make_proof: MakeProof,
    /// Must carry exactly one message, the [`OrderMarker`] of the order
    /// output and `payer`, which the SPP proof covers. The program checks it
    /// and forwards the transact data unchanged.
    pub spp_proof: TransactIxData,
}

impl Make {
    pub fn instruction(self) -> Result<Instruction> {
        let Self {
            payer,
            tree,
            make_proof,
            spp_proof,
        } = self;

        // The owner signers: the maker for the source it spends, then the
        // maker's order authority for the data-bearing order output, which
        // the program signs for during the CPI.
        let transact = Transact {
            payer,
            input_trees: vec![tree; spp_proof.tree_contexts.len()],
            output_tree: tree,
            owner_signers: vec![payer],
            interface_transfer_accounts: Vec::new(),
            data: spp_proof,
        };
        let (order_authority, authority_bump) = order_authority_pda_and_bump(&payer);
        let mut accounts = vec![AccountMeta::new(payer, true)];
        accounts.extend(transact.cpi_accounts(&[order_authority]));

        let serialized_ix = wincode::serialize(&MakeIxData {
            proof: make_proof,
            authority_bump,
            transact: transact.data,
        })
        .map_err(err)?;
        let mut instruction_data = vec![tag::MAKE];
        instruction_data.extend_from_slice(&serialized_ix);
        Ok(Instruction {
            program_id: swap_program::ID,
            accounts,
            data: instruction_data,
        })
    }
}

#[cfg(test)]
mod tests {
    use solana_keypair::Keypair;
    use spr_keypair::shielded::ShieldedKeypair;
    use spr_transaction::{
        instructions::transact::{PrivateTxHash, Shape, SppProofOutputUtxo},
        utxo::{SppProofInputUtxo, Utxo},
        Data,
    };

    use super::*;

    // TODO(tree-id): resolve the tree id from the tree account.
    const OUTPUT_TREE_ID: u16 = 0;

    fn data_hash_bytes(byte: u8) -> [u8; 32] {
        let mut out = [byte; 32];
        out[0] = 0;
        out
    }

    #[test]
    fn sign_order_utxo_make_layout() {
        let owner_keypair = ShieldedKeypair::from_keypair(&Keypair::new_from_array([7u8; 32]))
            .expect("owner keypair");
        let order_keypair = ShieldedKeypair::from_keypair(&Keypair::new_from_array([9u8; 32]))
            .expect("order keypair");
        let taker_keypair = ShieldedKeypair::from_keypair(&Keypair::new_from_array([13u8; 32]))
            .expect("market maker keypair");

        let input_amount = 1_000_000u64;
        let order_utxo_amount = 400_000u64;

        let input_utxo = Utxo {
            owner: owner_keypair.signing_pubkey(),
            asset: spr_transaction::Mint::SOL,
            amount: input_amount,
            blinding: crate::shared::test_blinding(5),
            ring_program_id: None,
            data: Data::default(),
        };
        let input_utxo = crate::shared::test_input_utxo(
            input_utxo,
            &owner_keypair.nullifier_key,
            OUTPUT_TREE_ID,
        );

        let order_utxo = SppProofOutputUtxo {
            owner_address: Some(order_keypair.shielded_address().expect("order address")),
            asset: spr_transaction::Mint::SOL,
            amount: order_utxo_amount,
            blinding: crate::shared::test_blinding(11),
            ..Default::default()
        }
        .with_utxo_data(vec![1, 2, 3, 4], data_hash_bytes(0xAB));

        let taker_address = taker_keypair
            .shielded_address()
            .expect("market maker address");
        let owner_address = owner_keypair.shielded_address().expect("owner address");

        let change_amount = input_amount - order_utxo_amount;
        let change =
            SppProofOutputUtxo::new(spr_transaction::Mint::SOL, change_amount, owner_address)
                .expect("change output");
        let input_utxos = vec![
            input_utxo,
            SppProofInputUtxo::dummy(OUTPUT_TREE_ID).unwrap(),
        ];
        let mut spp_proof_inputs = crate::shared::test_transaction(
            input_utxos,
            vec![change, order_utxo],
            &owner_keypair,
            OUTPUT_TREE_ID,
        );
        let order_utxo = spp_proof_inputs.output_utxos.get(1).expect("order output");
        let order_utxo_hash = order_utxo.hash(OUTPUT_TREE_ID).expect("order hash");
        let marker_message = OrderMarker {
            order_utxo_hash,
            maker_pubkey: Pubkey::default(),
            taker_address,
        }
        .message()
        .expect("marker message");
        let expected_marker_bytes = borsh::to_vec(&MarkerData {
            order_utxo_hash,
            maker_pubkey: Pubkey::default().to_bytes(),
        })
        .expect("marker bytes");
        spp_proof_inputs.external_data.messages.push(marker_message);

        assert_eq!(
            spp_proof_inputs.check_shape().expect("shape"),
            Shape::IN2_OUT2
        );
        assert_eq!(spp_proof_inputs.output_utxos.len(), 2);

        let change = spp_proof_inputs
            .output_utxos
            .first()
            .expect("change output");
        assert!(!change.is_dummy());
        assert_eq!(change.amount, input_amount - order_utxo_amount);
        let order_output_utxo = spp_proof_inputs.output_utxos.get(1).expect("order output");
        assert!(!order_output_utxo.is_dummy());

        let change_hash = change.hash(OUTPUT_TREE_ID).expect("change hash");
        let output_hashes: Vec<[u8; 32]> = spp_proof_inputs
            .external_data
            .outputs
            .iter()
            .map(|output| output.utxo_hash)
            .collect();
        assert_eq!(output_hashes, vec![change_hash, order_utxo_hash]);

        let marker = spp_proof_inputs
            .external_data
            .messages
            .first()
            .expect("marker message");
        assert_eq!(spp_proof_inputs.external_data.messages.len(), 1);
        assert_eq!(marker.data, expected_marker_bytes);

        assert_eq!(spp_proof_inputs.input_utxos.len(), 2);
        let input_utxo = spp_proof_inputs.input_utxos.first().expect("input");
        assert!(!input_utxo.is_dummy());
        assert!(spp_proof_inputs
            .input_utxos
            .get(1)
            .expect("dummy input")
            .is_dummy());
        let source_input_hash = input_utxo.hash();

        let external_data_hash = spp_proof_inputs
            .external_data
            .hash()
            .expect("external data hash");
        let expected = PrivateTxHash::new(
            &[source_input_hash, [0u8; 32]],
            &[change_hash, order_utxo_hash],
            &spp_proof_inputs
                .private_tx_blinding()
                .expect("private tx blinding"),
        )
        .hash()
        .expect("private tx hash");
        assert_eq!(
            spr_keypair::hash::sha256(&[expected, external_data_hash].concat()),
            spp_proof_inputs.message_hash().expect("message hash")
        );
    }

    #[test]
    fn sign_order_utxo_make_zero_change_utxo() {
        let owner_keypair = ShieldedKeypair::from_keypair(&Keypair::new_from_array([3u8; 32]))
            .expect("owner keypair");
        let order_keypair = ShieldedKeypair::from_keypair(&Keypair::new_from_array([4u8; 32]))
            .expect("order keypair");
        let taker_keypair = ShieldedKeypair::from_keypair(&Keypair::new_from_array([14u8; 32]))
            .expect("market maker keypair");

        let amount = 250_000u64;
        let input_utxo = Utxo {
            owner: owner_keypair.signing_pubkey(),
            asset: spr_transaction::Mint::SOL,
            amount,
            blinding: crate::shared::test_blinding(6),
            ring_program_id: None,
            data: Data::default(),
        };
        let input_utxo = crate::shared::test_input_utxo(
            input_utxo,
            &owner_keypair.nullifier_key,
            OUTPUT_TREE_ID,
        );

        let order_utxo = SppProofOutputUtxo {
            owner_address: Some(order_keypair.shielded_address().expect("order address")),
            asset: spr_transaction::Mint::SOL,
            amount,
            blinding: crate::shared::test_blinding(12),
            ..Default::default()
        }
        .with_utxo_data(vec![9, 9], data_hash_bytes(0xCD));

        let taker_address = taker_keypair
            .shielded_address()
            .expect("market maker address");
        let owner_address = owner_keypair.shielded_address().expect("owner address");

        let change = SppProofOutputUtxo::new(spr_transaction::Mint::SOL, 0, owner_address)
            .expect("change output");
        let input_utxos = vec![
            input_utxo,
            SppProofInputUtxo::dummy(OUTPUT_TREE_ID).unwrap(),
        ];
        let mut spp_proof_inputs = crate::shared::test_transaction(
            input_utxos,
            vec![change, order_utxo],
            &owner_keypair,
            OUTPUT_TREE_ID,
        );
        let order_utxo = spp_proof_inputs.output_utxos.get(1).expect("order output");
        let order_utxo_hash = order_utxo.hash(OUTPUT_TREE_ID).expect("order hash");
        let marker_message = OrderMarker {
            order_utxo_hash,
            maker_pubkey: Pubkey::default(),
            taker_address,
        }
        .message()
        .expect("marker message");
        spp_proof_inputs.external_data.messages.push(marker_message);

        let change = spp_proof_inputs
            .output_utxos
            .first()
            .expect("change output");
        assert!(!change.is_dummy());
        assert_eq!(change.amount, 0);

        let order_output_utxo = spp_proof_inputs.output_utxos.get(1).expect("order output");
        let external_data_hash = spp_proof_inputs
            .external_data
            .hash()
            .expect("external data hash");
        let input_utxo = spp_proof_inputs.input_utxos.first().expect("input");
        let source_input_hash = input_utxo.hash();
        let expected = PrivateTxHash::new(
            &[source_input_hash, [0u8; 32]],
            &[
                change.hash(OUTPUT_TREE_ID).expect("change hash"),
                order_output_utxo.hash(OUTPUT_TREE_ID).expect("order hash"),
            ],
            &spp_proof_inputs
                .private_tx_blinding()
                .expect("private tx blinding"),
        )
        .hash()
        .expect("private tx hash");
        let message_hash = spp_proof_inputs.message_hash().expect("message hash");
        assert_eq!(
            spr_keypair::hash::sha256(&[expected, external_data_hash].concat()),
            message_hash
        );
    }
}
