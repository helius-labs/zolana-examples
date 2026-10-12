//! Settlement witness parity with the program, SPP leg parity with the
//! program's leg builders, and a full settlement proof against the VK.
#[allow(dead_code)]
#[path = "../examples/settlement_fixture.rs"]
mod fixture;

use p256::SecretKey;
use private_staking_program::{
    instructions::proof,
    math::PoolSnapshot,
    settlement::{
        self, SettlementAmounts, SettlementLegs, SppProofData, PAYOUT_CIRCUIT, WITHDRAWAL_CIRCUIT,
    },
    state::{BatchHeader, BATCH_DISCRIMINATOR},
};
use private_staking_sdk::{
    encryption::{decrypt_payout, Context, Recipient},
    instructions::PoolAccounts,
    note::recipient_address,
    settlement::SettlementProofInputs,
};
use solana_address::Address;
use spl_stake_pool::state::Fee;
use zolana_client::{MerkleContext, MerkleProof, NonInclusionProof, SpendProof};
use zolana_client::{NULLIFIER_TREE_HEIGHT, STATE_TREE_HEIGHT};
use zolana_interface::instruction::instruction_data::transact::{
    TransactProof, TreeContext, NO_UTXO_ROOT,
};

fn decimal_u64(map: &private_staking_prover::ProofInputMap, key: &str) -> u64 {
    map.get(key)
        .and_then(|values| values.first())
        .and_then(|value| value.parse().ok())
        .unwrap_or_else(|| panic!("witness has no u64 {key}"))
}

fn decimal_field(map: &private_staking_prover::ProofInputMap, key: &str) -> String {
    map.get(key)
        .and_then(|values| values.first())
        .cloned()
        .unwrap_or_else(|| panic!("witness has no {key}"))
}

/// The witness-side pool and amounts rebuild the context's pool and batch
/// hashes, and the program's `proof_context` (from a batch header and the
/// legs) reproduces the SDK's context and the witness public input.
fn assert_context_parity(fixture: &fixture::Fixture) {
    let builder = &fixture.builder;
    let inputs = builder.proof_inputs().unwrap();
    let witness = inputs.witness().unwrap();

    let pool = PoolSnapshot {
        total_lamports: decimal_u64(&witness, "Context_Pool_Lamports"),
        pool_token_supply: decimal_u64(&witness, "Context_Pool_Supply"),
        deposit_fee: Fee {
            numerator: decimal_u64(&witness, "Context_Pool_DepositNumerator"),
            denominator: decimal_u64(&witness, "Context_Pool_DepositDenominator"),
        },
        withdrawal_fee: Fee {
            numerator: decimal_u64(&witness, "Context_Pool_WithdrawalNumerator"),
            denominator: decimal_u64(&witness, "Context_Pool_WithdrawalDenominator"),
        },
    };
    let amounts = SettlementAmounts {
        escrow_sol: decimal_u64(&witness, "Context_Amounts_EscrowSol"),
        escrow_pool_tokens: decimal_u64(&witness, "Context_Amounts_EscrowPoolTokens"),
        payout_sol: decimal_u64(&witness, "Context_Amounts_PayoutSol"),
        payout_pool_tokens: decimal_u64(&witness, "Context_Amounts_PayoutPoolTokens"),
    };
    let entries = builder.batch_entries();
    let entries_hash =
        settlement::entries_hash(&entries, &builder.tx_viewing_pk(), &[5; 16]).unwrap();
    assert_eq!(
        settlement::pool_hash(&pool).unwrap(),
        inputs.context.pool_hash
    );
    assert_eq!(
        settlement::batch_hash(&amounts, &entries_hash).unwrap(),
        inputs.context.batch_hash
    );

    let batch = BatchHeader {
        discriminator: BATCH_DISCRIMINATOR,
        epoch: fixture.epoch,
        hash_chain_index: 3,
        utxo_hash_chain: builder.utxo_hash_chain().unwrap(),
        intent_count: u16::try_from(entries.len()).unwrap(),
        crank: fixture.config.crank,
        rent_payer: fixture.payer,
        tx_viewing_pk: builder.tx_viewing_pk(),
        salt: [5; 16],
        filled_count: u16::try_from(entries.len()).unwrap(),
    };
    let proof = |byte: u8| TransactProof {
        a: [byte; 32],
        b: [byte; 128],
        c: [byte; 32],
    };
    let legs = builder.cached_utxo().map(|cached| {
        (
            SettlementLegs {
                amounts,
                escrow_tree_context: TreeContext {
                    utxo_tree_root_index: 0,
                    nullifier_tree_root_index: 0,
                },
                payout_nullifier_root_index: 0,
                withdrawal_output_seed: builder.withdrawal_output_seed().unwrap(),
                spend: SppProofData {
                    proof: proof(1),
                    private_tx_hash: builder.spend_private_tx_hash().unwrap(),
                },
                payout: SppProofData {
                    proof: proof(2),
                    private_tx_hash: builder.payout_private_tx_hash().unwrap(),
                },
            },
            cached,
        )
    });
    let program = settlement::proof_context(
        &fixture.config_key,
        &fixture.config,
        &fixture.epoch,
        &batch,
        &entries,
        fixture::TREE_ID,
        &pool,
        legs.as_ref().map(|(legs, cached)| (legs, cached)),
    )
    .unwrap();
    assert_eq!(program, inputs.context);
    assert_eq!(
        decimal_field(&witness, "PublicInputHash"),
        zolana_gnark_ffi_prover::decimal(&program.hash().unwrap())
    );
}

#[test]
fn settlement_context_matches_the_program() {
    for intents in [fixture::four(), fixture::sixteen(), fixture::cancelled()] {
        assert_context_parity(&fixture::fixture(&intents).unwrap());
    }
}

#[test]
fn absent_slots_copy_slot_zero() {
    let fixture = fixture::fixture(&fixture::four()).unwrap();
    let witness = fixture.builder.proof_inputs().unwrap().witness().unwrap();
    for (name, expected) in [
        ("Present", Some("0")),
        ("Cancelled", Some("0")),
        ("PoolAsset", None),
        ("Amount", None),
        ("Blinding", None),
        ("NullifierPK", None),
        ("PayoutBlinding", None),
        ("SigningPK", None),
        ("UserPoint", None),
    ] {
        let slot0 = witness.get(&format!("Escrow0_{name}")).unwrap();
        for index in 4..16 {
            let slot = witness.get(&format!("Escrow{index}_{name}")).unwrap();
            match expected {
                Some(value) => assert_eq!(slot, &vec![value.to_string()], "{name}"),
                None => assert_eq!(slot, slot0, "{name}"),
            }
        }
    }
    // The cancelled entry has no payout blinding; its recipient point is uncompressed.
    assert_eq!(
        witness.get("Escrow1_PayoutBlinding").unwrap(),
        &vec!["0".to_string()]
    );
    let viewing = fixture.recipients.first().unwrap().viewing_pubkey();
    let point: Vec<String> = fixture::uncompressed(viewing.as_bytes())
        .unwrap()
        .iter()
        .map(u8::to_string)
        .collect();
    assert_eq!(witness.get("Escrow0_UserPoint").unwrap(), &point);
}

fn context(tree: Address, tree_type: u16) -> MerkleContext {
    MerkleContext { tree_type, tree }
}

fn non_inclusion(leaf: [u8; 32], tree: Address) -> NonInclusionProof {
    NonInclusionProof {
        leaf,
        merkle_context: context(tree, 1),
        path: vec![[0; 32]; NULLIFIER_TREE_HEIGHT],
        low_element: [0; 32],
        low_element_index: 0,
        high_element: [0; 32],
        high_element_index: 0,
        root: [9; 32],
        root_seq: 0,
        root_index: 4,
    }
}

/// Both SPP legs, assembled without proving, equal the program's leg
/// builders field by field and carry the locally computed private tx hashes.
#[test]
fn spp_legs_match_the_program_leg_builders() {
    let fixture = fixture::fixture(&fixture::four()).unwrap();
    let builder = &fixture.builder;
    let tree = fixture.config.escrow_tree;
    let spend_proofs = builder
        .spend_targets()
        .into_iter()
        .map(|(leaf, nullifier, leaf_index)| SpendProof {
            state: MerkleProof {
                leaf,
                merkle_context: context(tree, 0),
                path: vec![[0; 32]; STATE_TREE_HEIGHT],
                leaf_index,
                root: [7; 32],
                root_seq: 0,
                root_index: 3,
            },
            nullifier: non_inclusion(nullifier, tree),
        })
        .collect();
    let padding = builder
        .withdrawal_padding_nullifiers()
        .unwrap()
        .into_iter()
        .map(|nullifier| non_inclusion(nullifier, tree))
        .collect();
    let spend = builder
        .withdrawal_request(spend_proofs, padding)
        .unwrap()
        .assemble()
        .unwrap()
        .transaction;
    let cached = builder.cached_utxo().unwrap();
    let payout = builder
        .payout_request(non_inclusion(cached.nullifier, tree))
        .unwrap()
        .assemble()
        .unwrap()
        .transaction;
    assert_eq!(spend.data.circuit, WITHDRAWAL_CIRCUIT);
    assert_eq!(payout.data.circuit, PAYOUT_CIRCUIT);
    assert_eq!(
        spend.data.tree_contexts,
        vec![TreeContext {
            utxo_tree_root_index: 3,
            nullifier_tree_root_index: 4
        }]
    );
    assert_eq!(
        payout.data.tree_contexts,
        vec![TreeContext {
            utxo_tree_root_index: NO_UTXO_ROOT,
            nullifier_tree_root_index: 4
        }]
    );
    assert_eq!(spend.data.inputs.len(), 3);
    assert_eq!(payout.data.outputs.len(), 3);
    let bump = zolana_interface::pda::spl_interface_bump(fixture.config.pool_mint.as_array());
    let inputs = builder.settlement_inputs(&spend, &payout, bump).unwrap();
    assert_eq!(
        inputs.context.spend_private_tx_hash,
        spend.data.private_tx_hash
    );
    assert_eq!(
        inputs.context.payout_private_tx_hash,
        payout.data.private_tx_hash
    );

    // The stored payouts decrypt for each recipient and for the crank.
    let crank_secret =
        SecretKey::from_slice(&fixture.crank.viewing_key.secret_bytes()[..]).unwrap();
    let mut active = 0u32;
    for ((entry, payout_note), recipient) in builder
        .batch_entries()
        .iter()
        .zip(builder.payout_notes())
        .zip(&fixture.recipients)
    {
        let (Some(output), Some(payout_note)) = (&entry.1, payout_note) else {
            continue;
        };
        let context = Context {
            tx_viewing_pk: builder.tx_viewing_pk(),
            salt: [5; 16],
            output_index: active,
            utxo_hash: output.utxo_hash,
        };
        let address = recipient_address(&recipient.shielded_address().unwrap());
        let user_secret = SecretKey::from_slice(&recipient.viewing_key.secret_bytes()[..]).unwrap();
        for (secret, role) in [
            (&user_secret, Recipient::User),
            (&crank_secret, Recipient::Crank),
        ] {
            assert_eq!(
                decrypt_payout(
                    output.data.as_deref().unwrap(),
                    secret,
                    role,
                    &context,
                    &address,
                    fixture.config.pool_mint,
                    fixture::POOL_ASSET_ID,
                )
                .unwrap(),
                payout_note
            );
        }
        active += 1;
    }
    assert_eq!(
        builder
            .payout_notes()
            .iter()
            .map(|note| note.map(|note| note.amount))
            .collect::<Vec<_>>(),
        vec![Some(29), None, Some(20), Some(70)]
    );

    // execute_batch: 10 fixed + 4 nullifier PDAs + 9 SPP + 8 pool accounts.
    let ix = builder
        .execute_batch(
            private_staking_program::instructions::proof::LogicProof {
                a: [0; 32],
                b: [0; 64],
                c: [0; 32],
                commitment: [0; 32],
                commitment_pok: [0; 32],
            },
            Some((spend, payout)),
            &PoolAccounts {
                stake_pool_program: spl_stake_pool::id(),
                stake_pool: fixture.config.stake_pool,
                withdraw_authority: Address::new_from_array([40; 32]),
                reserve_stake: Address::new_from_array([41; 32]),
                manager_fee_account: Address::new_from_array([42; 32]),
            },
        )
        .unwrap();
    assert_eq!(ix.accounts.len(), 31);
    assert!(ix.accounts.iter().skip(1).all(|meta| !meta.is_signer));
}

fn verify(inputs: &SettlementProofInputs, proof: &proof::LogicProof) -> bool {
    proof::verify(
        proof,
        inputs.context.hash().unwrap(),
        &private_staking_program::verifying_keys::settlement::VERIFYINGKEY,
    )
    .is_ok()
}

#[test]
fn proves_and_verifies_a_four_entry_settlement() {
    let fixture = fixture::fixture(&fixture::four()).unwrap();
    let mut inputs = fixture.builder.proof_inputs().unwrap();
    let start = std::time::Instant::now();
    let proof = inputs.prove().unwrap();
    println!("settlement proof in {:?}", start.elapsed());
    assert!(verify(&inputs, &proof), "settlement proof must verify");
    inputs.context.intent_count += 1;
    assert!(
        !verify(&inputs, &proof),
        "a mutated context must not verify"
    );
}
