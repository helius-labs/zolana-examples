use p256::{elliptic_curve::sec1::ToEncodedPoint, SecretKey};
use private_staking_program::{
    instructions::proof,
    pda,
    state::{Config, IntentUtxoData, ShieldedAddress, CONFIG_DISCRIMINATOR},
    verifying_keys::intent::VERIFYINGKEY,
};
use private_staking_prover::{CircuitId, PROVER};
use private_staking_sdk::{
    encryption::{self, Context, Recipient},
    intent::{FundingInput, IntentProofInputs, IntentProofParams},
    note::{recipient_address, Note},
};
use solana_address::Address;
use zolana_client::ProofInputUtxo;
use zolana_hasher::primitives::right_align;
use zolana_interface::instruction::instruction_data::transact::{
    CircuitId as SppCircuit, InputUtxo, OwnerTag, TransactIxData, TransactOutput, TransactProof,
    TreeContext,
};
use zolana_keypair::{NullifierKey, PublicKey, ShieldedKeypair};
use zolana_transaction::instructions::transact::PrivateTxHash;

fn fixture(
    unregistered_escrow: bool,
    escrow_funding: bool,
) -> (IntentProofInputs, Vec<u8>, SecretKey, Context) {
    let user = ShieldedKeypair::new_ed25519().unwrap();
    let user_view = SecretKey::from_slice(&[2; 32]).unwrap();
    let crank_view = SecretKey::from_slice(&[3; 32]).unwrap();
    let ephemeral = SecretKey::from_slice(&[4; 32]).unwrap();
    let mut recipient = recipient_address(&user.shielded_address().unwrap());
    recipient.viewing_pubkey = user_view
        .public_key()
        .to_encoded_point(true)
        .as_bytes()
        .try_into()
        .unwrap();
    let pool = Address::new_from_array([12; 32]);
    let config_key = pda::config(&pool).0;
    let epoch = pda::epoch(&config_key, 42, 0).0;
    let authority = pda::escrow_authority(&config_key).0;
    let config = Config {
        discriminator: CONFIG_DISCRIMINATOR,
        authority: Address::new_from_array([13; 32]),
        stake_pool_program_id: spl_stake_pool::id(),
        stake_pool: pool,
        pool_mint: Address::new_from_array([14; 32]),
        token_program_id: Address::new_from_array([15; 32]),
        escrow_tree: Address::new_from_array([16; 32]),
        crank: ShieldedAddress {
            signing_pubkey: Address::new_from_array([17; 32]),
            nullifier_pubkey: right_align(&[4]),
            viewing_pubkey: crank_view
                .public_key()
                .to_encoded_point(true)
                .as_bytes()
                .try_into()
                .unwrap(),
        },
        intent_fee_lamports: 50_000,
        settlement_window_seconds: 600,
    };
    let note = Note {
        version: 1,
        tree_id: 7,
        asset: Address::default(),
        asset_id: 1,
        amount: 10_000_000,
        blinding: right_align(&[19]),
        signing_pubkey: *PublicKey::from_pda(&authority).as_bytes(),
        nullifier_pubkey: NullifierKey::from_secret([0; 31]).pubkey().unwrap(),
        intent: Some(IntentUtxoData { epoch, recipient }),
    };
    let source_pk = if escrow_funding {
        PublicKey::from_pda(&authority)
    } else {
        user.signing_pubkey()
    };
    let source_nf_pk = if escrow_funding {
        note.nullifier_pubkey
    } else {
        recipient.nullifier_pubkey
    };
    let source_owner_hash = zolana_keypair::hash::owner_hash(&source_pk, &source_nf_pk).unwrap();
    let source = FundingInput {
        utxo: ProofInputUtxo::new(
            source_owner_hash,
            &Address::default(),
            100_000_000,
            &right_align(&[20]),
            5,
        )
        .unwrap(),
        owner_identity: source_pk.owner_proof_input_hash().unwrap(),
        nullifier_pk: source_nf_pk,
    };
    let escrow_utxo = ProofInputUtxo::new(
        zolana_keypair::hash::owner_hash(&PublicKey::from_pda(&authority), &note.nullifier_pubkey)
            .unwrap(),
        &note.asset,
        note.amount,
        &note.blinding,
        note.tree_id,
    )
    .unwrap()
    .with_data_hash(note.data_hash().unwrap());
    let mut change = ProofInputUtxo::new(
        source_owner_hash,
        &Address::default(),
        90_000_000,
        &right_align(&[21]),
        7,
    )
    .unwrap();
    if unregistered_escrow {
        change.owner_hash = escrow_utxo.owner_hash;
        change.data_hash = escrow_utxo.data_hash;
    }
    let blinding = right_align(&[22]);
    let private_tx_hash = PrivateTxHash::new(
        &[source.utxo.hash().unwrap(), [0; 32]],
        &[note.hash().unwrap(), change.hash().unwrap()],
        &blinding,
    )
    .hash()
    .unwrap();
    let encryption_context = Context {
        tx_viewing_pk: ephemeral
            .public_key()
            .to_encoded_point(true)
            .as_bytes()
            .try_into()
            .unwrap(),
        salt: [5; 16],
        output_index: 0,
        utxo_hash: note.hash().unwrap(),
    };
    let ciphertext = encryption::encrypt(
        &note,
        &ephemeral,
        &recipient.viewing_pubkey,
        &config.crank.viewing_pubkey,
        &encryption_context,
    )
    .unwrap();
    let spp = TransactIxData {
        expiry_unix_ts: 1_000,
        tx_viewing_pk: encryption_context.tx_viewing_pk,
        salt: encryption_context.salt,
        interface_transfers: vec![],
        data_hash: None,
        ring_data_hash: None,
        messages: vec![],
        private_tx_hash,
        circuit: SppCircuit::ConfidentialEddsa(2, 2, 2),
        proof: TransactProof::zeroed(),
        inputs: vec![InputUtxo {
            nullifier_hash: right_align(&[23]),
            tree_index: 0,
        }],
        tree_contexts: vec![TreeContext {
            utxo_tree_root_index: 1,
            nullifier_tree_root_index: 0,
        }],
        outputs: vec![
            TransactOutput {
                utxo_hash: note.hash().unwrap(),
                owner_tag: OwnerTag::Inline(authority.to_bytes()),
                data: Some(ciphertext.clone()),
            },
            TransactOutput {
                utxo_hash: change.hash().unwrap(),
                owner_tag: OwnerTag::Inline(source_pk.confidential_view_tag().unwrap()),
                data: None,
            },
        ],
    };
    let inputs = IntentProofInputs::new(IntentProofParams {
        config_key,
        config: &config,
        epoch,
        spp: &spp,
        escrow: note,
        funding: [source, FundingInput::default()],
        change,
        private_tx_blinding: blinding,
        ephemeral,
    })
    .unwrap();
    (inputs, ciphertext, user_view, encryption_context)
}

#[test]
fn intent_proof_verifies_and_rejects_authority_funding_and_unregistered_escrows() {
    let dir = PROVER.keys_dir(CircuitId::Intent);
    if !dir.join("pk.bin").exists() {
        PROVER
            .setup_insecure_test_keys(CircuitId::Intent, &dir)
            .unwrap();
    }
    let (inputs, ciphertext, viewing_secret, context) = fixture(false, false);
    let proof = inputs.prove().unwrap();
    proof::verify(&proof, inputs.context.hash().unwrap(), &VERIFYINGKEY).unwrap();
    assert_eq!(
        encryption::decrypt(&ciphertext, &viewing_secret, Recipient::User, &context).unwrap(),
        inputs.escrow
    );
    let mut changed = inputs.context;
    changed.tree_id += 1;
    assert!(proof::verify(&proof, changed.hash().unwrap(), &VERIFYINGKEY).is_err());
    changed = inputs.context;
    changed.epoch_hash = right_align(&[99]);
    assert!(proof::verify(&proof, changed.hash().unwrap(), &VERIFYINGKEY).is_err());
    assert!(
        fixture(true, false).0.prove().is_err(),
        "an extra epoch-linked output would bypass registration"
    );
    assert!(
        fixture(false, true).0.prove().is_err(),
        "create_intent must not authorize spending an existing escrow"
    );
}
