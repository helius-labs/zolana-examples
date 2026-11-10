use solana_address::Address;
use solana_instruction::AccountMeta;
use spr_interface::{
    instruction::{
        instruction_data::transact::{
            InputUtxo, OwnerTag, TransactIxData, TransactOutput, TreeContext,
        },
        CircuitId, TransactProof,
    },
    N_PUBLIC_SLOTS,
};
use spr_program::instruction::Transact;
use swap_sdk::{
    instructions::take::Take, order_authority_pda_and_bump, tag, TakeIxData, TakeProof,
};

#[test]
fn take_forwards_the_order_and_paying_taker_as_owner_signers() {
    let payer = Address::new_unique();
    let maker = Address::new_unique();
    let tree = Address::new_unique();
    let (order_authority, authority_bump) = order_authority_pda_and_bump(&maker);
    let data = TransactIxData {
        expiry_unix_ts: u64::MAX,
        private_tx_hash: [0; 32],
        circuit: CircuitId::ConfidentialSigner(2, 2, N_PUBLIC_SLOTS as u8),
        tx_viewing_pk: [2; 33],
        salt: [0; 16],
        proof: TransactProof::zeroed(),
        inputs: [1, 2]
            .map(|value| InputUtxo {
                nullifier_hash: [value; 32],
                tree_index: 0,
            })
            .to_vec(),
        interface_transfers: Vec::new(),
        data_hash: None,
        ring_data_hash: None,
        outputs: [3, 4]
            .map(|value| TransactOutput {
                utxo_hash: [value; 32],
                owner_tag: OwnerTag::Inline([value; 32]),
                data: None,
            })
            .to_vec(),
        messages: Vec::new(),
        tree_contexts: vec![TreeContext {
            utxo_tree_root_index: 0,
            nullifier_tree_root_index: 0,
        }],
    };
    let mut expected = Transact {
        payer,
        input_trees: vec![tree],
        output_tree: tree,
        owner_signers: vec![order_authority, payer],
        interface_transfer_accounts: Vec::new(),
        data: data.clone(),
    }
    .instruction()
    .accounts;
    // The swap program supplies this signature during CPI, not at transaction entry.
    for account in &mut expected {
        if account.pubkey == order_authority {
            account.is_signer = false;
        }
    }
    let take_proof = TakeProof {
        proof_a: [0; 32],
        proof_b: [0; 64],
        proof_c: [0; 32],
    };
    let mut expected_data = vec![tag::TAKE];
    expected_data.extend(
        wincode::serialize(&TakeIxData {
            proof: take_proof,
            maker: maker.to_bytes(),
            authority_bump,
            transact: data.clone(),
        })
        .expect("serialize take"),
    );
    let instruction = Take {
        payer,
        maker,
        tree,
        spp_proof: data,
        take_proof,
    }
    .instruction()
    .expect("take");
    assert_eq!(instruction.data, expected_data);
    assert_eq!(
        instruction.accounts.first(),
        Some(&AccountMeta::new(payer, true))
    );
    assert_eq!(instruction.accounts.get(1..), Some(expected.as_slice()));
}
