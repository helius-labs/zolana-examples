use crate::shared::*;
use timelock_escrow_program::verifier::escrow::{PublicInputs, Verifier, VERIFYINGKEY};
use timelock_escrow_sdk::{escrow_authority, proof::CircuitId, state::escrow_utxo};

const CIRCUIT: CircuitId = CircuitId::Escrow;

#[test]
fn program_vk_has_no_commitment() {
    assert_eq!(VERIFYINGKEY.nr_pubinputs, 1);
    assert!(
        VERIFYINGKEY.vk_commitment.is_none(),
        "the escrow circuit is standard Groth16: no BSB22 commitment"
    );
    assert_eq!(
        VERIFYINGKEY.vk_ic.len(),
        2,
        "standard Groth16 vk_ic length must be public_inputs + 1"
    );
}

#[test]
fn escrow_prove_verify() {
    // 1. Check the outputs: the change to the creator, then the escrow UTXO
    // owned by the escrow-authority PDA, whose data decodes to the terms.
    let inputs = escrow_inputs(250, 750);
    let finalized = inputs.create_finalized_transaction().unwrap();
    let outputs = finalized.output_utxos();
    let change = outputs.first().unwrap();
    let escrow = outputs.get(1).unwrap();
    let terms = terms(&inputs.creator);
    assert_eq!(
        (
            change
                .owner_address
                .map(|address| address.owner_hash().unwrap()),
            change.asset,
            change.amount,
            change.data_hash,
            escrow
                .owner_address
                .map(|address| address.owner_hash().unwrap()),
            escrow.asset,
            escrow.amount,
            escrow_utxo(escrow, OUTPUT_TREE_ID, 0).unwrap().data,
            outputs.len(),
        ),
        (
            Some(inputs.sender().owner_hash().unwrap()),
            mint(),
            750,
            None,
            Some(escrow_authority().owner_hash().unwrap()),
            mint(),
            250,
            terms,
            2,
        )
    );

    // 2. Prove, and verify with the program's verifier against the
    // transaction's private_tx_hash.
    let (_, proof) = prove_and_verify(CIRCUIT, &inputs, &VERIFYINGKEY);
    let private_tx_hash = finalized.padding_independent_private_tx_hash().unwrap();
    let signer = field_bytes(owner_pk_field(&inputs.creator));
    verify(&proof, signer, &private_tx_hash).expect(
        "the committed escrow VERIFYINGKEY must accept the proof; run `zolana zk compile --test`",
    );

    // 3. Another signer or a tampered private_tx_hash fails.
    let mut other_signer = signer;
    other_signer[31] ^= 0x01;
    let mut tampered = private_tx_hash;
    tampered[31] ^= 0x01;
    assert!(verify(&proof, other_signer, &private_tx_hash).is_err());
    assert!(verify(&proof, signer, &tampered).is_err());
}

#[test]
fn escrow_rejects_zero_amount() {
    assert_invalid(CIRCUIT, &escrow_inputs(0, 750));
}

#[test]
fn escrow_zero_change_proves() {
    let inputs = escrow_inputs(250, 0);
    let finalized = inputs.create_finalized_transaction().unwrap();
    let change = finalized.output_utxos().first().unwrap();
    assert_eq!(
        (change.amount, change.compact),
        (0, false),
        "a zero-value change output is non-dummy: its real utxo hash enters private_tx_hash, matching SPP"
    );
    prove_and_verify(CIRCUIT, &inputs, &VERIFYINGKEY);
}

#[test]
fn escrow_rejects_wrong_signer_amount_and_source() {
    // 1. Tamper the signer, the locked amount or the source data. The source
    // commitment itself is bound by the SPP proof, not this circuit.
    let mutations: [fn(&mut Escrow); 3] = [
        |inputs| inputs.program.public.owner_pk_field = field(99),
        |inputs| inputs.program.private.amount = 1_001,
        |inputs| inputs.program.private.source.data_hash = field(99),
    ];
    // 2. Check each mutation is unprovable.
    for mutate in mutations {
        let mut inputs = escrow_inputs(250, 750);
        mutate(&mut inputs);
        assert_invalid(CIRCUIT, &inputs);
    }
}

#[test]
fn escrow_proof_rejects_a_different_escrow_output() {
    // 1. Prove the escrow.
    let inputs = escrow_inputs(250, 750);
    let finalized = inputs.create_finalized_transaction().unwrap();
    let (_, proof) = prove_and_verify(CIRCUIT, &inputs, &VERIFYINGKEY);
    let change = finalized.output_utxos().first().unwrap();
    let escrow = finalized.output_utxos().get(1).unwrap();
    let source = field_bytes(inputs.program.private.source.hash().unwrap());

    // 2. Recompute the SPP hash with another data hash or another amount
    // locked.
    for other_terms in [false, true] {
        let mut output = escrow.clone();
        if other_terms {
            output.data_hash = Some(fe(99));
        } else {
            output.amount -= 1;
        }
        let hash = spr_program::PrivateTxHash::new(
            &[source],
            &[
                change.hash(OUTPUT_TREE_ID).unwrap(),
                output.hash(OUTPUT_TREE_ID).unwrap(),
            ],
            &finalized.private_tx_blinding().unwrap(),
        )
        .hash()
        .unwrap();
        assert!(verify(&proof, field_bytes(owner_pk_field(&inputs.creator)), &hash).is_err());
    }
}

fn verify(
    proof: &spr_program::CompressedProof,
    owner_pk_field: [u8; 32],
    private_tx_hash: &[u8; 32],
) -> Result<(), spr_program::verifier::VerifyError> {
    Verifier {
        proof,
        public_inputs: PublicInputs { owner_pk_field },
        private_tx_hash,
        transfers: &[],
    }
    .verify()
}
