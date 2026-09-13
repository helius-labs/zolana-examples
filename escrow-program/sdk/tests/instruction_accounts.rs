use solana_instruction::AccountMeta;
use solana_pubkey::Pubkey;
use timelock_escrow_sdk::{
    escrow_authority_pda,
    instructions::{escrow::Escrow, withdraw::Withdraw},
    EscrowProof, WithdrawProof,
};
use zolana_interface::{
    instruction::instruction_data::transact::{CircuitId, TransactIxData, TransactProof},
    SHIELDED_POOL_PROGRAM_ID,
};

fn payload() -> TransactIxData {
    TransactIxData {
        expiry_unix_ts: 0,
        private_tx_hash: [0; 32],
        circuit: CircuitId::ConfidentialEddsa(2, 2, 3),
        tx_viewing_pk: [0; 33],
        salt: [0; 16],
        proof: TransactProof::zeroed(),
        inputs: vec![],
        interface_transfers: vec![],
        data_hash: None,
        ring_data_hash: None,
        outputs: vec![],
        messages: vec![],
    }
}

fn spp_accounts(payer: Pubkey, input: Pubkey, output: Pubkey) -> Vec<AccountMeta> {
    vec![
        AccountMeta::new(payer, true),
        AccountMeta::new(input, false),
        AccountMeta::new(output, false),
        AccountMeta::new_readonly(Pubkey::new_from_array(SHIELDED_POOL_PROGRAM_ID), false),
        AccountMeta::new_readonly(Pubkey::default(), false),
        AccountMeta::new_readonly(escrow_authority_pda(), false),
    ]
}

#[test]
fn builders_preserve_spp_order_and_privileges() {
    let payer = Pubkey::new_from_array([1; 32]);
    let creator = Pubkey::new_from_array([2; 32]);
    let input = Pubkey::new_from_array([3; 32]);
    // Distinct addresses detect swapped tree positions; this is not an
    // execution test of separate-tree support.
    for output in [input, Pubkey::new_from_array([4; 32])] {
        let escrow = Escrow {
            payer,
            input_tree: input,
            output_tree: output,
            escrow_proof: EscrowProof {
                proof_a: [0; 32],
                proof_b: [0; 64],
                proof_c: [0; 32],
            },
            spp_proof: payload(),
        }
        .instruction()
        .unwrap();
        assert_eq!(escrow.accounts[0], AccountMeta::new(payer, true));
        assert_eq!(&escrow.accounts[1..], spp_accounts(payer, input, output));

        let withdraw = Withdraw {
            creator,
            payer,
            input_tree: input,
            output_tree: output,
            withdraw_proof: WithdrawProof {
                proof_a: [0; 32],
                proof_b: [0; 64],
                proof_c: [0; 32],
            },
            unlock_timestamp: 1,
            spp_proof: payload(),
        }
        .instruction()
        .unwrap();
        assert_eq!(withdraw.accounts[0], AccountMeta::new(payer, true));
        assert_eq!(
            withdraw.accounts[1],
            AccountMeta::new_readonly(creator, true)
        );
        assert_eq!(&withdraw.accounts[2..], spp_accounts(payer, input, output));
    }
}

#[test]
fn unlock_window_remains_strict() {
    use timelock_escrow_program::instructions::shared::check_after_window;
    assert!(check_after_window(-1, 10).is_err());
    assert!(check_after_window(9, 10).is_err());
    assert!(check_after_window(10, 10).is_err());
    assert!(check_after_window(11, 10).is_ok());
}
