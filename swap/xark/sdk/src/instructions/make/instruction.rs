use anyhow::Result;
use solana_instruction::{AccountMeta, Instruction};
use solana_pubkey::Pubkey;
use spr_interface::{
    instruction::instruction_data::transact::{MessageData, TransactIxData},
    SHIELDED_POOL_PROGRAM_ID,
};
use spr_keypair::ShieldedAddress;
use spr_program::instruction::nullifier_pda_accounts;
use spr_transaction::TransactionError;

use crate::{
    err, order_authority_bump, order_authority_pda, tag, MakeIxData, MakeProof, MarkerData,
};

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
    /// The maker: fee payer, marker signer and owner of the spent source.
    pub payer: Pubkey,
    /// The viewing key of the order UTXO's address, the taker's.
    pub viewing_pubkey: [u8; 33],
    pub tree: Pubkey,
    pub make_proof: MakeProof,
    pub spp_proof: TransactIxData,
}

impl Make {
    pub fn instruction(self) -> Result<Instruction> {
        let Self {
            payer,
            viewing_pubkey,
            tree,
            make_proof,
            spp_proof,
        } = self;

        let nullifier_pdas = nullifier_pda_accounts(
            &tree,
            spp_proof.inputs.iter().map(|input| &input.nullifier_hash),
        );
        let serialized_ix = wincode::serialize(&MakeIxData {
            proof: make_proof,
            viewing_pubkey,
            authority_bump: order_authority_bump(&payer),
            transact: spp_proof,
        })
        .map_err(err)?;

        let mut accounts = vec![
            AccountMeta::new(payer, true),
            AccountMeta::new(payer, true),
            AccountMeta::new(tree, false),
            AccountMeta::new_readonly(Pubkey::new_from_array(SHIELDED_POOL_PROGRAM_ID), false),
            AccountMeta::new_readonly(Pubkey::default(), false),
            AccountMeta::new(tree, false),
        ];
        accounts.extend(nullifier_pdas);
        // The owner signers: the maker for the source it spends, then the
        // maker's order authority for the data-bearing order output, which
        // the program signs for during the CPI.
        accounts.push(AccountMeta::new_readonly(payer, true));
        accounts.push(AccountMeta::new_readonly(
            order_authority_pda(&payer),
            false,
        ));
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
    use spr_keypair::ShieldedKeypair;

    use super::*;

    /// The marker message's data is the `MarkerData` bytes the program
    /// compares against, and its view tag is the taker's.
    #[test]
    fn order_marker_message_is_the_program_marker() {
        let taker_address = ShieldedKeypair::from_keypair(&Keypair::new_from_array([13u8; 32]))
            .expect("taker keypair")
            .shielded_address()
            .expect("taker address");
        let marker = MarkerData {
            order_utxo_hash: [5; 32],
            maker_pubkey: [7; 32],
        };
        let message = OrderMarker {
            order_utxo_hash: marker.order_utxo_hash,
            maker_pubkey: Pubkey::new_from_array(marker.maker_pubkey),
            taker_address,
        }
        .message()
        .expect("marker message");
        assert_eq!(
            message,
            MessageData {
                view_tag: taker_address
                    .signing_pubkey
                    .confidential_view_tag()
                    .expect("view tag"),
                data: borsh::to_vec(&marker).expect("serialize marker"),
            }
        );
    }
}
