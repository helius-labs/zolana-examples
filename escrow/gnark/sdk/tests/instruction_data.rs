use solana_address::Address;
use spr_interface::{
    instruction::{
        instruction_data::transact::{
            InputUtxo, OwnerTag, TransactIxData, TransactOutput, TreeContext,
        },
        CircuitId, TransactProof,
    },
    N_PUBLIC_SLOTS,
};
use timelock_escrow_sdk::{
    escrow_authority_pda_and_bump,
    instructions::{escrow::Escrow, withdraw::Withdraw},
    tag, EscrowIxData, EscrowProof, WithdrawIxData, WithdrawProof,
};

fn transact() -> TransactIxData {
    TransactIxData {
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
    }
}

/// The escrow instruction carries the creator's canonical escrow-authority
/// bump and the transact data unchanged.
#[test]
fn escrow_carries_the_canonical_authority_bump() {
    let creator = Address::new_unique();
    let (escrow_authority, authority_bump) = escrow_authority_pda_and_bump(&creator);
    let proof = EscrowProof {
        proof_a: [7; 32],
        proof_b: [8; 64],
        proof_c: [9; 32],
    };
    let mut expected = vec![tag::ESCROW];
    expected.extend(
        wincode::serialize(&EscrowIxData {
            proof,
            authority_bump,
            transact: transact(),
        })
        .expect("serialize escrow"),
    );
    let instruction = Escrow {
        creator,
        payer: Address::new_unique(),
        tree: Address::new_unique(),
        escrow_proof: proof,
        spp_proof: transact(),
    }
    .instruction()
    .expect("escrow");
    assert_eq!(instruction.data, expected);
    assert_eq!(
        instruction.accounts.last().map(|meta| meta.pubkey),
        Some(escrow_authority)
    );
}

/// The withdraw instruction carries the creator's canonical escrow-authority
/// bump.
#[test]
fn withdraw_carries_the_canonical_authority_bump() {
    let creator = Address::new_unique();
    let (escrow_authority, authority_bump) = escrow_authority_pda_and_bump(&creator);
    let proof = WithdrawProof {
        proof_a: [7; 32],
        proof_b: [8; 64],
        proof_c: [9; 32],
    };
    let mut expected = vec![tag::WITHDRAW];
    expected.extend(
        wincode::serialize(&WithdrawIxData {
            proof,
            unlock_timestamp: 5,
            authority_bump,
            transact: transact(),
        })
        .expect("serialize withdraw"),
    );
    let instruction = Withdraw {
        creator,
        payer: Address::new_unique(),
        tree: Address::new_unique(),
        withdraw_proof: proof,
        unlock_timestamp: 5,
        spp_proof: transact(),
    }
    .instruction()
    .expect("withdraw");
    assert_eq!(instruction.data, expected);
    assert_eq!(
        instruction.accounts.last().map(|meta| meta.pubkey),
        Some(escrow_authority)
    );
}
