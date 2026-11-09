use solana_address::Address;
use spr_keypair::ShieldedKeypair;
use spr_program::{to_bytes, CompressedProof};
use spr_test_utils::utxo::assign_output_blindings;
use spr_transaction::utxo::{
    derive_output_blinding_seed, derive_private_tx_blinding, derive_transact_output_blinding,
};
use swap_program::{
    circuits::Take,
    verifier::take::{PublicInputs, Verifier, VERIFYINGKEY},
};
use swap_sdk::{
    instructions::take::{take_blinding_seed, TakeProofInputParams},
    proof::CircuitId,
    prover::SwapProverClient,
    state::{OrderTerms, OrderUtxo},
};

use crate::shared::{assert_invalid, fe, field, private_tx_hash, INPUT_TREE_ID, OUTPUT_TREE_ID};

fn sample_params() -> TakeProofInputParams {
    let maker = ShieldedKeypair::new_ed25519()
        .unwrap()
        .shielded_address()
        .unwrap();
    let taker = ShieldedKeypair::new_ed25519()
        .unwrap()
        .shielded_address()
        .unwrap();
    let order_utxo = OrderUtxo {
        terms: OrderTerms {
            destination_mint: Address::new_from_array([2; 32]),
            destination_amount: 250,
            destination: maker,
            taker: taker.solana_address().unwrap(),
            expiry: 1_700_000_000,
        },
        blinding: fe(7),
        source_mint: spr_transaction::Mint::new(Address::new_from_array([1; 32]), 3),
        source_amount: 1_000,
        destination_asset_id: 2,
    };
    let first_nullifier = order_utxo
        .to_input_utxo(INPUT_TREE_ID, 0)
        .unwrap()
        .nullifier();
    let blinding_seed = take_blinding_seed(&order_utxo.blinding).unwrap();
    let seed = derive_output_blinding_seed(&first_nullifier, &blinding_seed).unwrap();
    let mut outputs = [
        order_utxo.source_output(taker, fe(0)),
        order_utxo.destination_output(maker, fe(0)),
    ];
    assign_output_blindings(&mut outputs, &first_nullifier, &seed).unwrap();
    let [source_output, destination_output] = outputs;
    TakeProofInputParams {
        taker_in: order_utxo.destination_output(taker, fe(13)),
        order_utxo,
        source_output,
        destination_output,
        private_tx_blinding: derive_private_tx_blinding(&first_nullifier, &blinding_seed).unwrap(),
        input_tree_id: INPUT_TREE_ID,
        output_tree_id: OUTPUT_TREE_ID,
    }
}

fn sample_inputs() -> Take {
    sample_params().to_proof_inputs().unwrap()
}

/// Recompute the private transaction hash after a mutation, so a negative
/// fails on the mutated constraint rather than on a stale hash.
fn refresh_hash(take: &mut Take) {
    let private = &take.private;
    take.private.private_tx_hash = private_tx_hash(
        &[Some(&private.order_utxo), Some(&private.taker_in)],
        &[&private.source_output, &private.destination_output],
        &private.private_tx_blinding,
    );
}

/// The program's verifier with the public inputs it reads from the
/// transact: its expiry, first nullifier and private transaction hash.
fn verify(
    proof: &CompressedProof,
    expiry: u64,
    first_nullifier: [u8; 32],
    private_tx_hash: [u8; 32],
) -> bool {
    Verifier {
        proof,
        public_inputs: PublicInputs {
            expiry,
            first_nullifier,
        },
        private_tx_hash: &private_tx_hash,
        transfers: &[],
    }
    .verify()
    .is_ok()
}

#[test]
fn program_vk_has_one_public_input() {
    assert_eq!(VERIFYINGKEY.nr_pubinputs, 1);
    assert_eq!(
        VERIFYINGKEY.vk_ic.len(),
        2,
        "standard Groth16 vk_ic length must be public_inputs + 1"
    );
}

#[test]
fn take_prove_verify_and_bind_public_inputs() {
    // 1. Prove a take.
    let take = sample_inputs();
    let proof = SwapProverClient::new().prove_take(&take).expect("prove");
    assert!(proof.a.iter().any(|byte| *byte != 0));

    // 2. The committed verifier accepts the transact's values.
    let expiry = take.public.expiry;
    let first_nullifier = to_bytes(&take.public.first_nullifier);
    let tx_hash = to_bytes(&take.private.private_tx_hash);
    assert!(verify(&proof, expiry, first_nullifier, tx_hash));

    // 3. It rejects another expiry, first nullifier or transaction.
    let mut tampered = tx_hash;
    tampered[31] ^= 0x01;
    assert!(!verify(&proof, expiry + 1, first_nullifier, tx_hash));
    assert!(!verify(&proof, expiry, fe(17), tx_hash));
    assert!(!verify(&proof, expiry, first_nullifier, tampered));
}

#[test]
fn take_rejects_unrecoverable_payouts_and_transaction_blinding() {
    for attack in 0..4 {
        let mut take = sample_inputs();
        let first_nullifier = to_bytes(&take.public.first_nullifier);
        match attack {
            0 => take.private.destination_output.blinding = field(&fe(29)),
            1 => take.private.source_output.blinding = field(&fe(29)),
            2 => take.private.private_tx_blinding = field(&fe(29)),
            _ => {
                // A self-consistent derivation under a seed other than the
                // order's.
                let blinding_seed = fe(29);
                let seed = derive_output_blinding_seed(&first_nullifier, &blinding_seed).unwrap();
                take.private.private_tx_blinding =
                    field(&derive_private_tx_blinding(&first_nullifier, &blinding_seed).unwrap());
                for (index, output) in [
                    &mut take.private.source_output,
                    &mut take.private.destination_output,
                ]
                .into_iter()
                .enumerate()
                {
                    output.blinding = field(
                        &derive_transact_output_blinding(&first_nullifier, &seed, index as u32)
                            .unwrap(),
                    );
                }
            }
        }
        refresh_hash(&mut take);
        assert_invalid(CircuitId::Take, &take, || take.execute());
    }
}

#[test]
fn take_rejects_wrong_payouts_and_terms() {
    // Tamper one payout, taker input or terms constraint at a time.
    let mutations: [fn(&mut Take); 9] = [
        |take| take.public.expiry += 1,
        |take| take.private.order.destination_amount = 0,
        |take| take.private.taker_in.amount = field(&fe(1)),
        |take| take.private.taker_in.asset = field(&fe(1)),
        |take| take.private.source_output.owner_hash = field(&fe(1)),
        |take| take.private.source_output.amount = field(&fe(1)),
        |take| take.private.destination_output.owner_hash = field(&fe(1)),
        |take| take.private.destination_output.asset = field(&fe(1)),
        |take| take.private.order_utxo.data_hash = field(&fe(1)),
    ];
    for mutate in mutations {
        let mut take = sample_inputs();
        mutate(&mut take);
        refresh_hash(&mut take);
        assert_invalid(CircuitId::Take, &take, || take.execute());
    }
}

#[test]
fn maker_recovers_payout_without_settlement_ciphertext() {
    let params = sample_params();
    let take = params.to_proof_inputs().unwrap();
    let first_nullifier = to_bytes(&take.public.first_nullifier);
    let destination_hash = to_bytes(&take.private.destination_output.hash().unwrap());
    let recovered = params
        .order_utxo
        .derived_destination_output(&first_nullifier)
        .unwrap();
    assert_eq!(recovered.hash(OUTPUT_TREE_ID).unwrap(), destination_hash);
    let wrong_first = params
        .order_utxo
        .derived_destination_output(&fe(17))
        .unwrap();
    assert_ne!(wrong_first.hash(OUTPUT_TREE_ID).unwrap(), destination_hash);
}

#[test]
fn sdk_rejects_a_taker_selected_blinding_seed_or_payout() {
    let mut params = sample_params();
    params.destination_output.blinding = fe(29);
    assert!(params
        .to_proof_inputs()
        .unwrap_err()
        .to_string()
        .contains("output 1 blinding"));
    let mut params = sample_params();
    params.private_tx_blinding = fe(29);
    assert!(params
        .to_proof_inputs()
        .unwrap_err()
        .to_string()
        .contains("blinding seed"));
}
