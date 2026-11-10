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
use timelock_escrow_sdk::{
    escrow_authority_pda,
    instructions::{escrow::Escrow, withdraw::Withdraw},
    EscrowProof,
};

const PROOF: EscrowProof = EscrowProof {
    a: [0; 32],
    b: [0; 64],
    c: [0; 32],
};

/// A transact of one input and two outputs.
fn transact() -> TransactIxData {
    TransactIxData {
        expiry_unix_ts: u64::MAX,
        private_tx_hash: [0; 32],
        circuit: CircuitId::ConfidentialSigner(1, 2, N_PUBLIC_SLOTS as u8),
        tx_viewing_pk: [2; 33],
        salt: [0; 16],
        proof: TransactProof::zeroed(),
        inputs: vec![InputUtxo {
            nullifier_hash: [1; 32],
            tree_index: 0,
        }],
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

/// The SPP CPI accounts of `data` paid by `payer`, with `owner_signers`,
/// ending in `creator`'s escrow-authority PDA, which the program signs for
/// inside the CPI.
fn spp_accounts(
    creator: &Address,
    payer: Address,
    tree: Address,
    owner_signers: Vec<Address>,
    data: TransactIxData,
) -> Vec<AccountMeta> {
    Transact {
        payer,
        input_trees: vec![tree],
        output_tree: tree,
        owner_signers,
        interface_transfer_accounts: Vec::new(),
        data,
    }
    .cpi_accounts(&[escrow_authority_pda(creator)])
}

fn escrow(creator: Address, payer: Address, tree: Address) -> Vec<AccountMeta> {
    Escrow {
        creator,
        payer,
        tree,
        escrow_proof: PROOF,
        viewing_pubkey: [2; 33],
        spp_proof: transact(),
    }
    .instruction()
    .expect("escrow")
    .accounts
}

/// Whoever pays, a separate fee payer or the creator itself, is the SPP
/// payer; the creator is the read-only signer the program reads and the owner
/// signer of the source it spends.
#[test]
fn escrow_forwards_the_payer_and_the_creator_as_owner_signer() {
    let creator = Address::new_unique();
    let tree = Address::new_unique();
    for (case, payer) in [
        ("separate payer", Address::new_unique()),
        ("creator", creator),
    ] {
        let mut expected = vec![AccountMeta::new_readonly(creator, true)];
        expected.extend(spp_accounts(
            &creator,
            payer,
            tree,
            vec![creator],
            transact(),
        ));
        assert_eq!(escrow(creator, payer, tree), expected, "{case} pays");
    }
}

/// The withdraw spends only the escrow UTXO, which the creator's
/// escrow-authority PDA owns: the creator signs for the program, not SPP.
#[test]
fn withdraw_forwards_no_owner_signer() {
    let creator = Address::new_unique();
    let payer = Address::new_unique();
    let tree = Address::new_unique();
    let mut expected = vec![
        AccountMeta::new(payer, true),
        AccountMeta::new_readonly(creator, true),
    ];
    expected.extend(spp_accounts(&creator, payer, tree, Vec::new(), transact()));
    let instruction = Withdraw {
        creator,
        payer,
        tree,
        withdraw_proof: PROOF,
        unlock_timestamp: 1,
        spp_proof: transact(),
    }
    .instruction()
    .expect("withdraw");
    assert_eq!(instruction.accounts, expected);
}
