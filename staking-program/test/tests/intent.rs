use mollusk_svm::Mollusk;
use num_bigint::BigUint;
use p256::SecretKey;
use private_staking_program::{
    instructions::intent::CreateIntentIxData,
    pda,
    state::{self, Config, EpochHeader, IntentUtxoData},
    ID,
};
use private_staking_sdk::{
    encryption::{self, Context, Recipient},
    instructions,
    intent::{FundingInput, IntentProofInputs, IntentProofParams},
    note::{crank_address, recipient_address, Note},
    spp::SppProofRequest,
};
use solana_account::Account;
use solana_address::Address;
use zolana_client::{
    MerkleContext, MerkleProof, NonInclusionProof, ProofInputUtxo, ProverClient, ProverLaunch,
    SpendProof, NULLIFIER_TREE_HEIGHT, STATE_TREE_HEIGHT,
};
use zolana_hasher::{primitives::right_align, Hasher, Poseidon};
use zolana_interface::{
    state::{
        default_tree_fees, discriminator::TREE_ACCOUNT_DISCRIMINATOR, nullifier_tree_params,
        tree_account_size, STATE_HEIGHT,
    },
    SHIELDED_POOL_PROGRAM_ID,
};
use zolana_keypair::{NullifierKey, ShieldedKeypair, ViewingKey};
use zolana_merkle_tree::{indexed::IndexedMerkleTree, MerkleTree};
use zolana_program::instruction::Transact;
use zolana_test_utils::utxo::{
    dummy_proofs, encrypt_transaction_data, prepare_output_blindings, wallet, ProofKeys,
};
use zolana_transaction::{
    instructions::transact::{SppProofInputs, SppProofOutputUtxo, BN254_MODULUS_DEC},
    utxo::{derive_private_tx_blinding, SppProofInputUtxo},
    Data, ExternalData, Mint, Utxo,
};
use zolana_tree::TreeAccount;

const STAKING_BINARY: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../target/deploy/private_staking_program"
);
const SPP_BINARY: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../.deps/zolana/target/deploy/shielded_pool_program"
);

fn wallet_account(lamports: u64) -> Account {
    Account {
        lamports,
        owner: Address::default(),
        ..Account::default()
    }
}

fn start_prover() {
    for circuit in [
        private_staking_prover::CircuitId::Intent,
        private_staking_prover::CircuitId::Cancel,
    ] {
        let dir = private_staking_prover::PROVER.keys_dir(circuit);
        if !dir.join("pk.bin").exists() {
            private_staking_prover::PROVER
                .setup_insecure_test_keys(circuit, &dir)
                .unwrap();
        }
    }
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    std::env::set_var("ZOLANA_PROVER_URL", "http://127.0.0.1:9873");
    std::env::set_var(
        "ZOLANA_PROVER_BIN",
        root.join(".deps/zolana/target/debug/zolana-prover"),
    );
    ProverLaunch::new_with_cli(root.join(".deps/zolana/target/debug/zolana"))
        .unwrap()
        .with_keys_dir(root.join("build/spp-keys"))
        .unwrap()
        .spawn()
        .unwrap();
}

fn tree_fixture(tree: Address, leaf: [u8; 32], nullifier: [u8; 32]) -> (Account, SpendProof) {
    let mut bytes = vec![0; tree_account_size()];
    let (utxo_root, nf_root) = {
        let mut account = TreeAccount::init(
            &mut bytes,
            TREE_ACCOUNT_DISCRIMINATOR,
            STATE_HEIGHT as u8,
            tree.to_bytes(),
            0,
            nullifier_tree_params(),
            default_tree_fees(nullifier_tree_params().input_queue_zkp_batch_size).unwrap(),
        )
        .unwrap();
        account.utxo_tree().append(leaf, 1).unwrap();
        (
            account.get_utxo_tree_root(1).unwrap(),
            account.get_nullifier_tree_root(0).unwrap(),
        )
    };
    let mut state = MerkleTree::<Poseidon>::new(STATE_TREE_HEIGHT, 0);
    state.append(&leaf).unwrap();
    assert_eq!(state.root(), utxo_root);
    let modulus = BigUint::parse_bytes(BN254_MODULUS_DEC.as_bytes(), 10).unwrap() - 1u32;
    let nf = IndexedMerkleTree::<Poseidon, usize>::new_with_next_value(
        NULLIFIER_TREE_HEIGHT,
        0,
        modulus,
    )
    .unwrap();
    assert_eq!(nf.root(), nf_root);
    let absence = nf
        .get_non_inclusion_proof(&BigUint::from_bytes_be(&nullifier))
        .unwrap();
    let context = MerkleContext { tree_type: 0, tree };
    let proof = SpendProof {
        state: MerkleProof {
            leaf,
            merkle_context: context.clone(),
            path: state.get_proof_of_leaf(0, true).unwrap().to_vec(),
            leaf_index: 0,
            root: utxo_root,
            root_seq: 0,
            root_index: 1,
        },
        nullifier: NonInclusionProof {
            leaf: nullifier,
            merkle_context: context,
            path: absence.merkle_proof.to_vec(),
            low_element: absence.leaf_lower_range_value,
            low_element_index: absence.leaf_index as u64,
            high_element: absence.leaf_higher_range_value,
            high_element_index: 0,
            root: nf_root,
            root_seq: 0,
            root_index: 0,
        },
    };
    (
        Account {
            lamports: 1_000_000_000_000,
            data: bytes,
            owner: Address::new_from_array(SHIELDED_POOL_PROGRAM_ID),
            ..Account::default()
        },
        proof,
    )
}

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
    note.blinding = outputs[0].blinding;
    let change = ProofInputUtxo::try_from((&outputs[1], 0)).unwrap();
    let ephemeral = SecretKey::from_slice(&[4; 32]).unwrap();
    let viewing_key = ViewingKey::from_secret_key(ephemeral.clone());
    let mut encoded = encrypt_transaction_data(&outputs, &viewing_key, 0).unwrap();
    let context = Context {
        tx_viewing_pk: *viewing_key.pubkey().as_bytes(),
        salt: encoded.salt,
        output_index: 0,
        utxo_hash: note.hash().unwrap(),
    };
    encoded.outputs[0].data = Some(
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
    let (tree_account, spend_proof) = tree_fixture(tree, source.utxo_hash, source_nullifier);
    let padding_proofs = dummy_proofs(&proof_inputs);
    let transaction = SppProofRequest {
        inputs: proof_inputs,
        spend_proofs: vec![spend_proof],
        padding_proofs,
        additional_signers: vec![authority],
    }
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
    let mut svm = Mollusk::new(&ID, STAKING_BINARY);
    let spp = Address::new_from_array(SHIELDED_POOL_PROGRAM_ID);
    svm.add_program(&spp, SPP_BINARY);
    svm.sysvars.clock.epoch = 42;
    svm.sysvars.clock.slot = svm.sysvars.epoch_schedule.get_first_slot_in_epoch(42);
    svm.sysvars.clock.unix_timestamp = 500;
    let mut cfg_data = vec![0; state::CONFIG_SIZE];
    state::write_config(&mut cfg_data, &config).unwrap();
    let mut epoch_data = vec![0; state::EPOCH_SIZE];
    let epoch_header = EpochHeader {
        discriminator: state::EPOCH_DISCRIMINATOR,
        config: config_key,
        epoch_id: 42,
        page_index: 0,
        submission_end_slot: svm.sysvars.epoch_schedule.get_first_slot_in_epoch(43),
        settlement_deadline: 0,
        locked: false,
        intent_count: 0,
        resolved_intent_count: 0,
        fee_balance_lamports: 0,
    };
    state::write_epoch(&mut epoch_data, &epoch_header).unwrap();
    let fixtures = [
        (payer, wallet_account(100_000_000_000)),
        (tree, tree_account),
        (
            config_key,
            Account {
                lamports: 1_000_000_000,
                data: cfg_data,
                owner: ID,
                ..Account::default()
            },
        ),
        (
            epoch_key,
            Account {
                lamports: 1_000_000_000,
                data: epoch_data,
                owner: ID,
                ..Account::default()
            },
        ),
    ];
    let keys: std::collections::BTreeSet<_> = ix.accounts.iter().map(|meta| meta.pubkey).collect();
    let accounts: Vec<_> = keys
        .into_iter()
        .map(|key| {
            if key == Address::default() {
                mollusk_svm::program::keyed_account_for_system_program()
            } else if key == spp {
                (
                    key,
                    mollusk_svm::program::create_program_account_loader_v3(&key),
                )
            } else {
                (
                    key,
                    fixtures
                        .iter()
                        .find(|(address, _)| *address == key)
                        .map(|(_, a)| a.clone())
                        .unwrap_or_else(|| wallet_account(0)),
                )
            }
        })
        .collect();
    let result = svm.process_instruction(&ix, &accounts);
    assert_eq!(result.raw_result, Ok(()));
    let updated_epoch = result
        .resulting_accounts
        .iter()
        .find(|(key, _)| *key == epoch_key)
        .unwrap();
    assert_eq!(
        state::epoch_from_bytes(&updated_epoch.1.data).unwrap(),
        EpochHeader {
            intent_count: 1,
            fee_balance_lamports: 50_000,
            ..epoch_header
        }
    );
    assert_eq!(updated_epoch.1.lamports, 1_000_050_000);
    assert_eq!(
        state::read_chain(&updated_epoch.1.data, 0).unwrap(),
        state::HashChain {
            utxo_hash_chain: Poseidon::hashv(&[&[0; 32], &note.hash().unwrap()]).unwrap(),
            intent_count: 1,
            executed: false,
            batch_exists: false,
        }
    );
    let nullifier_key = zolana_interface::pda::nullifier_pda(&tree, &source_nullifier).0;
    assert_eq!(
        result
            .resulting_accounts
            .iter()
            .find(|(key, _)| *key == nullifier_key)
            .unwrap()
            .1
            .owner,
        spp
    );
    let ciphertext = ix.data;
    let data: CreateIntentIxData = wincode::deserialize_exact(&ciphertext[1..]).unwrap();
    let user_viewing_secret = SecretKey::from_slice(&user.viewing_key.secret_bytes()[..]).unwrap();
    assert_eq!(
        encryption::decrypt(
            data.spp.outputs[0].data.as_ref().unwrap(),
            &user_viewing_secret,
            Recipient::User,
            &context
        )
        .unwrap(),
        note
    );
    let crank_viewing_secret =
        SecretKey::from_slice(&crank.viewing_key.secret_bytes()[..]).unwrap();
    assert_eq!(
        encryption::decrypt(
            data.spp.outputs[0].data.as_ref().unwrap(),
            &crank_viewing_secret,
            Recipient::Crank,
            &context
        )
        .unwrap(),
        note
    );
    exercise_cancellation(
        &mut svm,
        &result.resulting_accounts,
        &config,
        &user,
        &note,
        &data.spp,
    );
}

fn exercise_cancellation(
    svm: &mut Mollusk,
    accounts: &[(Address, Account)],
    config: &Config,
    user: &ShieldedKeypair,
    note: &Note,
    creation: &zolana_interface::instruction::instruction_data::transact::TransactIxData,
) {
    use private_staking_program::{error::StakingError, instructions::cancel::CancelIntentIxData};
    use private_staking_sdk::cancel::{CancelProofInputs, CancelProofParams};
    use solana_instruction::error::InstructionError;
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
    let mut tree_bytes = accounts
        .iter()
        .find(|(address, _)| *address == tree)
        .unwrap()
        .1
        .data
        .clone();
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
        creation.outputs[1].utxo_hash,
    ] {
        state_tree.append(&leaf).unwrap();
    }
    assert_eq!(state_tree.root(), root);
    let modulus = BigUint::parse_bytes(BN254_MODULUS_DEC.as_bytes(), 10).unwrap() - 1u32;
    let nf = IndexedMerkleTree::<Poseidon, usize>::new_with_next_value(
        NULLIFIER_TREE_HEIGHT,
        0,
        modulus,
    )
    .unwrap();
    let absence = nf
        .get_non_inclusion_proof(&BigUint::from_bytes_be(&nullifier))
        .unwrap();
    let context = MerkleContext { tree_type: 0, tree };
    let spend = SpendProof {
        state: MerkleProof {
            leaf: note.hash().unwrap(),
            merkle_context: context.clone(),
            path: state_tree.get_proof_of_leaf(1, true).unwrap().to_vec(),
            leaf_index: 1,
            root,
            root_seq: 0,
            root_index,
        },
        nullifier: NonInclusionProof {
            leaf: nullifier,
            merkle_context: context,
            path: absence.merkle_proof.to_vec(),
            low_element: absence.leaf_lower_range_value,
            low_element_index: absence.leaf_index as u64,
            high_element: absence.leaf_higher_range_value,
            high_element_index: 0,
            root: nf.root(),
            root_seq: 0,
            root_index: 0,
        },
    };
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
    refund.blinding = outputs[0].blinding;
    let ephemeral = SecretKey::from_slice(&[5; 32]).unwrap();
    let viewing = ViewingKey::from_secret_key(ephemeral.clone());
    let mut encoded = encrypt_transaction_data(&outputs[..1], &viewing, 0).unwrap();
    let cipher_context = Context {
        tx_viewing_pk: *viewing.pubkey().as_bytes(),
        salt: encoded.salt,
        output_index: 0,
        utxo_hash: refund.hash().unwrap(),
    };
    encoded.outputs[0].data = Some(
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
    let transaction = SppProofRequest {
        inputs: proof_inputs,
        spend_proofs: vec![spend],
        padding_proofs,
        additional_signers: vec![],
    }
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
    let mut full_accounts = accounts.to_vec();
    for meta in &ix.accounts {
        if !full_accounts.iter().any(|(key, _)| *key == meta.pubkey) {
            full_accounts.push((meta.pubkey, wallet_account(0)));
        }
    }
    let mut locked_accounts = full_accounts.clone();
    let epoch_account = locked_accounts
        .iter_mut()
        .find(|(address, _)| *address == epoch)
        .unwrap();
    let header = state::epoch_from_bytes(&epoch_account.1.data).unwrap();
    state::write_epoch(
        &mut epoch_account.1.data,
        &EpochHeader {
            locked: true,
            ..header
        },
    )
    .unwrap();
    assert_eq!(
        svm.process_instruction(&ix, &locked_accounts).raw_result,
        Err(InstructionError::Custom(StakingError::EpochLocked as u32))
    );
    let mut unauthorized = ix.clone();
    unauthorized.accounts[4].pubkey = config.crank.signing_pubkey;
    full_accounts.push((config.crank.signing_pubkey, wallet_account(1_000_000)));
    assert_eq!(
        svm.process_instruction(&unauthorized, &full_accounts)
            .raw_result,
        Err(InstructionError::Custom(
            StakingError::InvalidRecipient as u32
        ))
    );
    // Cancellation has no submission or settlement deadline while unlocked.
    svm.sysvars.clock.epoch = 44;
    svm.sysvars.clock.slot = svm.sysvars.epoch_schedule.get_first_slot_in_epoch(44);
    svm.sysvars.clock.unix_timestamp = 2_000;
    let result = svm.process_instruction(&ix, &full_accounts);
    assert_eq!(result.raw_result, Ok(()));
    let account = result
        .resulting_accounts
        .iter()
        .find(|(address, _)| *address == epoch)
        .unwrap();
    assert_eq!(
        state::epoch_from_bytes(&account.1.data).unwrap(),
        EpochHeader {
            resolved_intent_count: 1,
            ..header
        }
    );
    assert_eq!(
        account.1.lamports, 1_000_050_000,
        "the intent fee stays in the epoch"
    );
    let source_nullifier = zolana_interface::pda::nullifier_pda(&tree, &nullifier).0;
    assert_eq!(
        result
            .resulting_accounts
            .iter()
            .find(|(address, _)| *address == source_nullifier)
            .unwrap()
            .1
            .owner,
        Address::new_from_array(SHIELDED_POOL_PROGRAM_ID)
    );
    let viewing_secret = SecretKey::from_slice(&user.viewing_key.secret_bytes()[..]).unwrap();
    let encoded: CancelIntentIxData = wincode::deserialize_exact(&ix.data[1..]).unwrap();
    assert_eq!(
        encryption::decrypt(
            encoded.spp.outputs[0].data.as_ref().unwrap(),
            &viewing_secret,
            Recipient::User,
            &cipher_context
        )
        .unwrap(),
        refund
    );
}
