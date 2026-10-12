mod common;

use common::{start_prover, Harness, SPP};
use p256::SecretKey;
use private_staking_program::{
    instructions::intent::CreateIntentIxData,
    pda,
    state::{self, Config, EpochHeader, IntentUtxoData},
};
use private_staking_sdk::{
    encryption::{self, Context, Recipient},
    instructions,
    intent::{FundingInput, IntentProofInputs, IntentProofParams},
    note::{crank_address, recipient_address, Note},
    spp::SppProofRequest,
};
use solana_address::Address;
use zolana_client::{ProofInputUtxo, ProverClient, STATE_TREE_HEIGHT};
use zolana_hasher::{primitives::right_align, Hasher, Poseidon};
use zolana_keypair::{NullifierKey, ShieldedKeypair, ViewingKey};
use zolana_merkle_tree::MerkleTree;
use zolana_program::instruction::Transact;
use zolana_test_utils::utxo::{
    dummy_proofs, encrypt_transaction_data, prepare_output_blindings, wallet, ProofKeys,
};
use zolana_transaction::{
    instructions::transact::{SppProofInputs, SppProofOutputUtxo},
    utxo::{derive_private_tx_blinding, SppProofInputUtxo},
    Data, ExternalData, Mint, Utxo,
};
use zolana_tree::TreeAccount;

#[test]
fn creates_and_cancels_an_encrypted_intent_through_real_spp() {
    start_prover();
    let user = ShieldedKeypair::new_ed25519().unwrap();
    let payer = user.shielded_address().unwrap().solana_address().unwrap();
    let crank = ShieldedKeypair::new_ed25519().unwrap();
    let tree = zolana_interface::pda::tree(0);
    let pool = Address::new_from_array([12; 32]);
    let config_key = pda::config(&pool).0;
    let epoch_key = pda::epoch(&config_key, 42, 0).0;
    let authority = pda::escrow_authority(&config_key).0;
    let config = Config {
        discriminator: state::CONFIG_DISCRIMINATOR,
        authority: payer,
        stake_pool_program_id: spl_stake_pool::id(),
        stake_pool: pool,
        pool_mint: Address::new_from_array([14; 32]),
        token_program_id: Address::new_from_array([15; 32]),
        escrow_tree: tree,
        crank: crank_address(&crank.shielded_address().unwrap()).unwrap(),
        intent_fee_lamports: 50_000,
        settlement_window_seconds: 600,
        min_deposit_lamports: 1_000,
        min_withdrawal_pool_tokens: 1_000,
    };
    let source: SppProofInputUtxo = wallet(
        Utxo {
            owner: user.signing_pubkey(),
            asset: Mint::SOL,
            amount: 100_000_000,
            blinding: right_align(&[19]),
            ring_program_id: None,
            data: Data::default(),
        },
        &user.nullifier_key,
        0,
        0,
        None,
        None,
    )
    .unwrap()
    .into();
    let source_proof_utxo = ProofInputUtxo::try_from(&source).unwrap();
    let source_nullifier = source.nullifier();
    let mut note = Note {
        version: 1,
        tree_id: 0,
        asset: Mint::SOL.asset,
        asset_id: Mint::SOL.asset_id,
        amount: 10_000_000,
        blinding: right_align(&[20]),
        signing_pubkey: *zolana_keypair::PublicKey::from_pda(&authority).as_bytes(),
        nullifier_pubkey: NullifierKey::from_secret([0; 31]).pubkey().unwrap(),
        intent: Some(IntentUtxoData {
            epoch: epoch_key,
            recipient: recipient_address(&user.shielded_address().unwrap()),
        }),
    };
    let inputs = vec![source.clone(), SppProofInputUtxo::compact(0).unwrap()];
    let mut outputs = vec![
        note.output(user.viewing_pubkey()).unwrap(),
        SppProofOutputUtxo::new(Mint::SOL, 90_000_000, user.shielded_address().unwrap()).unwrap(),
    ];
    let blinding_seed = prepare_output_blindings(&inputs, &mut outputs).unwrap();
    note.blinding = outputs.first().unwrap().blinding;
    let change = ProofInputUtxo::try_from((outputs.get(1).unwrap(), 0)).unwrap();
    let ephemeral = SecretKey::from_slice(&[4; 32]).unwrap();
    let viewing_key = ViewingKey::from_secret_key(ephemeral.clone());
    let mut encoded = encrypt_transaction_data(&outputs, &viewing_key, 0).unwrap();
    let context = Context {
        tx_viewing_pk: *viewing_key.pubkey().as_bytes(),
        salt: encoded.salt,
        output_index: 0,
        utxo_hash: note.hash().unwrap(),
    };
    encoded.outputs.first_mut().unwrap().data = Some(
        encryption::encrypt(
            &note,
            &ephemeral,
            user.viewing_pubkey().as_bytes(),
            &config.crank.viewing_pubkey,
            &context,
        )
        .unwrap(),
    );
    let proof_inputs = SppProofInputs {
        input_utxos: inputs,
        output_utxos: encoded.output_utxos,
        external_data: ExternalData::new(
            context.tx_viewing_pk,
            encoded.salt,
            encoded.outputs,
            encoded.resolved_owner_tags,
            vec![],
        ),
        payer,
        blinding_seed,
        output_tree_id: 0,
        cache_accounts: Default::default(),
    };
    let mut h = Harness::new();
    let tree_fixture = h.tree(tree, &[source.utxo_hash]);
    let spend_proof = tree_fixture.spend_proof(tree, 0, source_nullifier);
    let padding_proofs = dummy_proofs(&proof_inputs);
    let transaction = SppProofRequest::new(
        proof_inputs,
        vec![spend_proof],
        padding_proofs,
        vec![authority],
    )
    .prove(&ProverClient::local(), &ProofKeys(&[&user.nullifier_key]))
    .unwrap();
    let intent_proof = IntentProofInputs::new(IntentProofParams {
        config_key,
        config: &config,
        epoch: epoch_key,
        spp: &transaction.data,
        escrow: note.clone(),
        funding: [
            FundingInput {
                utxo: source_proof_utxo,
                owner_identity: user.signing_pubkey().owner_proof_input_hash().unwrap(),
                nullifier_pk: user.nullifier_key.pubkey().unwrap(),
            },
            FundingInput::default(),
        ],
        change,
        private_tx_blinding: derive_private_tx_blinding(&source_nullifier, &blinding_seed).unwrap(),
        ephemeral,
    })
    .unwrap()
    .prove()
    .unwrap();
    let spp_accounts = Transact {
        payer,
        input_trees: transaction.input_trees,
        output_tree: tree,
        owner_signers: transaction.owner_signers,
        interface_transfer_accounts: vec![],
        data: transaction.data.clone(),
    }
    .instruction()
    .accounts;
    let ix = instructions::create_intent(
        payer,
        config_key,
        epoch_key,
        CreateIntentIxData {
            intent_proof,
            spp: transaction.data,
        },
        &spp_accounts,
    )
    .unwrap();
    h.put_config(&config);
    let epoch_header = EpochHeader {
        discriminator: state::EPOCH_DISCRIMINATOR,
        config: config_key,
        epoch_id: 42,
        page_index: 0,
        submission_end_slot: h.svm.sysvars.epoch_schedule.get_first_slot_in_epoch(43),
        settlement_deadline: 0,
        locked: false,
        intent_count: 0,
        resolved_intent_count: 0,
        fee_balance_lamports: 0,
    };
    h.put_epoch(epoch_key, &epoch_header);
    h.accounts.insert(payer, common::wallet(100_000_000_000));
    h.run(ix.clone(), Ok(()));
    let updated_epoch = h.account(&epoch_key);
    assert_eq!(
        state::epoch_from_bytes(&updated_epoch.data).unwrap(),
        EpochHeader {
            intent_count: 1,
            fee_balance_lamports: 50_000,
            ..epoch_header
        }
    );
    assert_eq!(updated_epoch.lamports, 1_000_050_000);
    assert_eq!(
        state::read_chain(&updated_epoch.data, 0).unwrap(),
        state::HashChain {
            utxo_hash_chain: Poseidon::hashv(&[&[0; 32], &note.hash().unwrap()]).unwrap(),
            intent_count: 1,
            executed: false,
            batch_exists: false,
        }
    );
    let nullifier_key = zolana_interface::pda::nullifier_pda(&tree, &source_nullifier).0;
    assert_eq!(h.account(&nullifier_key).owner, SPP);
    let ciphertext = ix.data;
    let data: CreateIntentIxData =
        wincode::deserialize_exact(ciphertext.get(1..).unwrap()).unwrap();
    let user_viewing_secret =
        SecretKey::from_slice(user.viewing_key.secret_bytes().as_slice()).unwrap();
    assert_eq!(
        encryption::decrypt(
            data.spp.outputs.first().unwrap().data.as_ref().unwrap(),
            &user_viewing_secret,
            Recipient::User,
            &context
        )
        .unwrap(),
        note
    );
    let crank_viewing_secret =
        SecretKey::from_slice(crank.viewing_key.secret_bytes().as_slice()).unwrap();
    assert_eq!(
        encryption::decrypt(
            data.spp.outputs.first().unwrap().data.as_ref().unwrap(),
            &crank_viewing_secret,
            Recipient::Crank,
            &context
        )
        .unwrap(),
        note
    );
    exercise_cancellation(&mut h, &config, &user, &note, &data.spp);
}

fn exercise_cancellation(
    h: &mut Harness,
    config: &Config,
    user: &ShieldedKeypair,
    note: &Note,
    creation: &zolana_interface::instruction::instruction_data::transact::TransactIxData,
) {
    use private_staking_program::{error::StakingError, instructions::cancel::CancelIntentIxData};
    use private_staking_sdk::cancel::{CancelProofInputs, CancelProofParams};
    let config_key = pda::config(&config.stake_pool).0;
    let epoch = note.intent.unwrap().epoch;
    let tree = config.escrow_tree;
    let payer = user.shielded_address().unwrap().solana_address().unwrap();
    let key = NullifierKey::from_secret([0; 31]);
    let input: SppProofInputUtxo = wallet(
        note.utxo().unwrap(),
        &key,
        note.tree_id,
        1,
        Some(note.data_hash().unwrap()),
        None,
    )
    .unwrap()
    .into();
    let nullifier = input.nullifier();
    let mut tree_bytes = h.account(&tree).data.clone();
    let mut actual_tree = TreeAccount::from_bytes(&mut tree_bytes, tree.to_bytes()).unwrap();
    let root_index = actual_tree.utxo_tree().current_root_index();
    let root = actual_tree.utxo_tree().root();
    let mut state_tree = MerkleTree::<Poseidon>::new(STATE_TREE_HEIGHT, 0);
    // The creation input precedes the escrow and change leaves.
    // Recover the first leaf from the fixture's known funding note.
    let funding = Utxo {
        owner: user.signing_pubkey(),
        asset: Mint::SOL,
        amount: 100_000_000,
        blinding: right_align(&[19]),
        ring_program_id: None,
        data: Data::default(),
    };
    let funding_hash = funding
        .hash(&user.nullifier_key.pubkey().unwrap(), &[0; 32], &[0; 32], 0)
        .unwrap();
    for leaf in [
        funding_hash,
        note.hash().unwrap(),
        creation.outputs.get(1).unwrap().utxo_hash,
    ] {
        state_tree.append(&leaf).unwrap();
    }
    assert_eq!(state_tree.root(), root);
    let spend = common::spend_proof(tree, &state_tree, root_index, 1, nullifier);
    let mut refund = Note {
        intent: None,
        signing_pubkey: *user.signing_pubkey().as_bytes(),
        nullifier_pubkey: user.nullifier_key.pubkey().unwrap(),
        ..note.clone()
    };
    let inputs = vec![input, SppProofInputUtxo::compact(0).unwrap()];
    let mut outputs = vec![
        refund.output(user.viewing_pubkey()).unwrap(),
        SppProofOutputUtxo {
            compact: true,
            ..Default::default()
        },
    ];
    let seed = prepare_output_blindings(&inputs, &mut outputs).unwrap();
    refund.blinding = outputs.first().unwrap().blinding;
    let ephemeral = SecretKey::from_slice(&[5; 32]).unwrap();
    let viewing = ViewingKey::from_secret_key(ephemeral.clone());
    let mut encoded = encrypt_transaction_data(outputs.get(..1).unwrap(), &viewing, 0).unwrap();
    let cipher_context = Context {
        tx_viewing_pk: *viewing.pubkey().as_bytes(),
        salt: encoded.salt,
        output_index: 0,
        utxo_hash: refund.hash().unwrap(),
    };
    encoded.outputs.first_mut().unwrap().data = Some(
        encryption::encrypt(
            &refund,
            &ephemeral,
            user.viewing_pubkey().as_bytes(),
            &config.crank.viewing_pubkey,
            &cipher_context,
        )
        .unwrap(),
    );
    let proof_inputs = SppProofInputs {
        input_utxos: inputs,
        output_utxos: outputs,
        external_data: ExternalData::new(
            cipher_context.tx_viewing_pk,
            encoded.salt,
            encoded.outputs,
            encoded.resolved_owner_tags,
            vec![],
        ),
        payer,
        blinding_seed: seed,
        output_tree_id: 0,
        cache_accounts: Default::default(),
    };
    let padding_proofs = dummy_proofs(&proof_inputs);
    let transaction = SppProofRequest::new(proof_inputs, vec![spend], padding_proofs, vec![])
        .prove(&ProverClient::local(), &ProofKeys(&[&key]))
        .unwrap();
    let inputs = CancelProofInputs::new(CancelProofParams {
        config_key,
        config,
        epoch,
        signer: payer,
        spp: &transaction.data,
        escrow: note.clone(),
        refund: refund.clone(),
        private_tx_blinding: derive_private_tx_blinding(&nullifier, &seed).unwrap(),
        ephemeral,
    })
    .unwrap();
    let proof = inputs.prove().unwrap();
    let mut wrong_signer = inputs.witness().unwrap();
    let mut context = inputs.context;
    context.signer_identity =
        zolana_hasher::primitives::solana_owner_identity(config.crank.signing_pubkey.as_array())
            .unwrap();
    wrong_signer.insert(
        "SignerIdentity".into(),
        vec![zolana_gnark_ffi_prover::decimal(&context.signer_identity)],
    );
    wrong_signer.insert(
        "PublicInputHash".into(),
        vec![zolana_gnark_ffi_prover::decimal(&context.hash().unwrap())],
    );
    assert!(
        private_staking_prover::PROVER
            .prove(private_staking_prover::CircuitId::Cancel, &wrong_signer)
            .is_err(),
        "the crank cannot prove recipient authorization"
    );
    let mut wrong_amount = inputs.witness().unwrap();
    wrong_amount.insert("Refund_Amount".into(), vec!["1".into()]);
    assert!(
        private_staking_prover::PROVER
            .prove(private_staking_prover::CircuitId::Cancel, &wrong_amount)
            .is_err(),
        "cancellation must refund the complete amount"
    );
    let spp_accounts = Transact {
        payer,
        input_trees: transaction.input_trees,
        output_tree: tree,
        owner_signers: transaction.owner_signers,
        interface_transfer_accounts: vec![],
        data: transaction.data.clone(),
    }
    .instruction()
    .accounts;
    let ix = instructions::cancel_intent(
        payer,
        payer,
        config_key,
        epoch,
        CancelIntentIxData {
            cancel_proof: proof,
            spp: transaction.data,
        },
        &spp_accounts,
    )
    .unwrap();
    let header = h.epoch_state();
    let unlocked = h.account(&epoch).data.clone();
    state::write_epoch(
        &mut h.accounts.get_mut(&epoch).unwrap().data,
        &EpochHeader {
            locked: true,
            ..header
        },
    )
    .unwrap();
    h.error(ix.clone(), StakingError::EpochLocked);
    h.accounts.get_mut(&epoch).unwrap().data = unlocked;
    let mut unauthorized = ix.clone();
    unauthorized.accounts.get_mut(4).unwrap().pubkey = config.crank.signing_pubkey;
    h.accounts
        .insert(config.crank.signing_pubkey, common::wallet(1_000_000));
    h.error(unauthorized, StakingError::InvalidRecipient);
    // Cancellation has no submission or settlement deadline while unlocked.
    h.svm.sysvars.clock.epoch = 44;
    h.svm.sysvars.clock.slot = h.svm.sysvars.epoch_schedule.get_first_slot_in_epoch(44);
    h.svm.sysvars.clock.unix_timestamp = 2_000;
    h.run(ix.clone(), Ok(()));
    let account = h.account(&epoch);
    assert_eq!(
        state::epoch_from_bytes(&account.data).unwrap(),
        EpochHeader {
            resolved_intent_count: 1,
            ..header
        }
    );
    assert_eq!(
        account.lamports, 1_000_050_000,
        "the intent fee stays in the epoch"
    );
    let source_nullifier = zolana_interface::pda::nullifier_pda(&tree, &nullifier).0;
    assert_eq!(h.account(&source_nullifier).owner, SPP);
    let viewing_secret = SecretKey::from_slice(user.viewing_key.secret_bytes().as_slice()).unwrap();
    let encoded: CancelIntentIxData =
        wincode::deserialize_exact(ix.data.get(1..).unwrap()).unwrap();
    assert_eq!(
        encryption::decrypt(
            encoded.spp.outputs.first().unwrap().data.as_ref().unwrap(),
            &viewing_secret,
            Recipient::User,
            &cipher_context
        )
        .unwrap(),
        refund
    );
}
