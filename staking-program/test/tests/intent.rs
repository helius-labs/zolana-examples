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
fn creates_an_encrypted_intent_through_the_real_spp_cpi() {
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
}
