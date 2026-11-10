use crate::shared::*;
use timelock_escrow_program::verifier::withdraw::{PublicInputs, Verifier, VERIFYINGKEY};
use timelock_escrow_sdk::proof::CircuitId;

const CIRCUIT: CircuitId = CircuitId::Withdraw;

#[test]
fn program_vk_has_no_commitment() {
    assert_eq!(VERIFYINGKEY.nr_pubinputs, 1);
    assert!(
        VERIFYINGKEY.vk_commitment.is_none(),
        "the withdraw circuit is standard Groth16: no BSB22 commitment"
    );
    assert_eq!(
        VERIFYINGKEY.vk_ic.len(),
        2,
        "standard Groth16 vk_ic length must be public_inputs + 1"
    );
}

#[test]
fn withdraw_prove_verify() {
    // 1. Check the whole escrow goes to the creator, with compact output
    // padding.
    let inputs = withdraw_inputs();
    let finalized = inputs.create_finalized_transaction().unwrap();
    let source_output = finalized.output_utxos().first().unwrap();
    assert_eq!(
        (
            source_output.owner_address,
            source_output.asset,
            source_output.amount,
            source_output.data_hash,
            source_output.compact,
            finalized.output_utxos().last().unwrap().compact,
        ),
        (Some(inputs.sender()), mint(), 1_000, None, false, true)
    );

    // 2. Prove, and verify with the program's verifier from the instruction's
    // unlock timestamp and the creator signer.
    let (_, proof) = prove_and_verify(CIRCUIT, &inputs, &VERIFYINGKEY);
    let private_tx_hash = finalized.padding_independent_private_tx_hash().unwrap();
    let public = public_inputs(&inputs);
    verify(&proof, public_inputs(&inputs), &private_tx_hash).expect(
        "the committed withdraw VERIFYINGKEY must accept the proof; run `zolana zk compile --test`",
    );

    // 3. Another unlock timestamp, another signer or another transaction
    // fails.
    let mut other_signer = public.owner_pk_field;
    other_signer[31] ^= 0x01;
    let mut tampered = private_tx_hash;
    tampered[31] ^= 0x01;
    for (public, private_tx_hash) in [
        (
            PublicInputs {
                unlock: UNLOCK + 1,
                owner_pk_field: public.owner_pk_field,
            },
            private_tx_hash,
        ),
        (
            PublicInputs {
                unlock: UNLOCK,
                owner_pk_field: other_signer,
            },
            private_tx_hash,
        ),
        (public_inputs(&inputs), tampered),
    ] {
        assert!(verify(&proof, public, &private_tx_hash).is_err());
    }
}

#[test]
fn withdraw_rejects_wrong_unlock_signer_terms_and_escrow() {
    // 1. Tamper the revealed unlock, the signer, the committed terms or the
    // escrow UTXO.
    let mutations: [fn(&mut Withdraw); 6] = [
        |inputs| inputs.program.public.unlock = UNLOCK + 1,
        |inputs| inputs.program.private.terms.unlock = UNLOCK + 1,
        |inputs| inputs.program.public.owner_pk_field = field(99),
        |inputs| inputs.program.private.terms.creator = terms(&creator()).creator,
        |inputs| inputs.program.private.escrow.data_hash = field(99),
        |inputs| inputs.program.private.escrow.amount = 0,
    ];
    // 2. Check each mutation is unprovable.
    for mutate in mutations {
        let mut inputs = withdraw_inputs();
        mutate(&mut inputs);
        assert_invalid(CIRCUIT, &inputs);
    }
}

#[test]
fn withdraw_proof_rejects_wrong_source_output() {
    // 1. Prove the withdraw.
    let inputs = withdraw_inputs();
    let finalized = inputs.create_finalized_transaction().unwrap();
    let (_, proof) = prove_and_verify(CIRCUIT, &inputs, &VERIFYINGKEY);
    let escrow = field_bytes(inputs.program.private.escrow.hash().unwrap());

    // 2. Recompute the SPP hash for a payout to another owner, a partial
    // payout and another asset.
    for case in 0..3 {
        let mut output = finalized.output_utxos().first().unwrap().clone();
        match case {
            0 => output.owner_address = Some(creator().shielded_address().unwrap()),
            1 => output.amount -= 1,
            _ => output.asset = spr_transaction::Mint::SOL,
        }
        let hash = spr_program::PrivateTxHash::new(
            &[escrow],
            &[output.hash(OUTPUT_TREE_ID).unwrap(), [0; 32]],
            &finalized.private_tx_blinding().unwrap(),
        )
        .hash()
        .unwrap();
        assert!(verify(&proof, public_inputs(&inputs), &hash).is_err());
    }
}

fn public_inputs(inputs: &Withdraw) -> PublicInputs {
    PublicInputs {
        unlock: inputs.program.public.unlock,
        owner_pk_field: field_bytes(inputs.program.public.owner_pk_field),
    }
}

fn verify(
    proof: &spr_program::CompressedProof,
    public_inputs: PublicInputs,
    private_tx_hash: &[u8; 32],
) -> Result<(), spr_program::verifier::VerifyError> {
    Verifier {
        proof,
        public_inputs,
        private_tx_hash,
        transfers: &[],
    }
    .verify()
}
