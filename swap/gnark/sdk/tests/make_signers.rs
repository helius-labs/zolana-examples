use solana_address::Address;
use solana_instruction::AccountMeta;
use spr_interface::{
    instruction::{
        instruction_data::transact::{
            InputUtxo, MessageData, OwnerTag, TransactIxData, TransactOutput, TreeContext,
        },
        CircuitId, TransactProof,
    },
    N_PUBLIC_SLOTS,
};
use spr_program::instruction::Transact;
use swap_sdk::{
    instructions::make::Make, order_authority_pda_and_bump, tag, MakeIxData, MakeProof, MarkerData,
};

/// SPP lists the owners of real inputs, then the owners of data-bearing
/// outputs, as owner signers: the maker for the source it spends, then its
/// order-authority PDA for the order output. The instruction data carries the
/// canonical bump and the client's marker unchanged.
#[test]
fn make_forwards_the_maker_then_its_order_authority_as_owner_signers() {
    let maker = Address::new_unique();
    let tree = Address::new_unique();
    let (order_authority, authority_bump) = order_authority_pda_and_bump(&maker);
    let marker = MarkerData {
        order_utxo_hash: [4; 32],
        maker_pubkey: maker.to_bytes(),
    };
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
        messages: vec![MessageData {
            view_tag: [5; 32],
            data: borsh::to_vec(&marker).expect("serialize marker"),
        }],
        tree_contexts: vec![TreeContext {
            utxo_tree_root_index: 0,
            nullifier_tree_root_index: 0,
        }],
    };
    let mut expected = Transact {
        payer: maker,
        input_trees: vec![tree],
        output_tree: tree,
        owner_signers: vec![maker, order_authority],
        interface_transfer_accounts: Vec::new(),
        data: data.clone(),
    }
    .instruction()
    .accounts;
    // The swap program supplies this signature during CPI, not at transaction entry.
    for account in expected.iter_mut().filter(|a| a.pubkey == order_authority) {
        account.is_signer = false;
    }
    let make_proof = MakeProof {
        proof_a: [0; 32],
        proof_b: [0; 64],
        proof_c: [0; 32],
    };
    let mut expected_data = vec![tag::MAKE];
    expected_data.extend(
        wincode::serialize(&MakeIxData {
            proof: make_proof,
            authority_bump,
            transact: data.clone(),
        })
        .expect("serialize make"),
    );
    let instruction = Make {
        payer: maker,
        tree,
        spp_proof: data,
        make_proof,
    }
    .instruction()
    .expect("make");
    assert_eq!(instruction.data, expected_data);
    assert_eq!(
        instruction.accounts.first(),
        Some(&AccountMeta::new(maker, true))
    );
    assert_eq!(instruction.accounts.get(1..), Some(expected.as_slice()));
}
