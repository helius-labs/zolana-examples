//! The make transaction the SDK builder produces: its SPP shape, the order of
//! its outputs and its one message, the taker's marker. The program checks
//! the marker and reads the order UTXO hash from `ORDER_SLOT`.
use solana_keypair::Keypair;
use spr_interface::{instruction::instruction_data::transact::MessageData, shape::Shape};
use spr_keypair::ShieldedKeypair;
use spr_transaction::{Data, Mint, Utxo};
use swap_sdk::{
    instructions::make::{MakeTransactionParams, CHANGE_SLOT, ORDER_SLOT},
    shared::identity,
    state::OrderTerms,
    MarkerData,
};

const TREE_ID: u16 = 7;
const SOURCE: u64 = 1_000;
const LOCKED: u64 = 400;

fn keypair(seed: u8) -> ShieldedKeypair {
    ShieldedKeypair::from_keypair(&Keypair::new_from_array([seed; 32])).unwrap()
}

#[test]
fn make_transaction_spends_one_source_into_change_then_order_with_one_marker() {
    let maker = keypair(7);
    let taker = keypair(13);
    let maker_address = maker.shielded_address().unwrap();
    let taker_address = taker.shielded_address().unwrap();
    let maker_pubkey = maker_address.solana_address().unwrap();
    let source_mint = Mint::new(solana_address::Address::new_from_array([3; 32]), 2);
    let utxo = |amount, blinding| Utxo {
        owner: maker.signing_pubkey(),
        asset: source_mint,
        amount,
        blinding,
        ring_program_id: None,
        data: Data::default(),
    };
    let source = spr_test_utils::utxo::wallet(
        utxo(SOURCE, [5; 32]),
        &maker.nullifier_key,
        TREE_ID,
        0,
        None,
        None,
    )
    .unwrap();
    let terms = OrderTerms {
        destination_mint: Mint::SOL,
        destination_amount: 250,
        maker: maker_address,
        expiry: 2_000_000_000,
        taker: Some(identity(&taker_address.solana_address().unwrap()).unwrap()),
    };

    let transaction = MakeTransactionParams {
        terms,
        source_amount: LOCKED,
        source,
        taker_address,
        payer: maker_pubkey,
    }
    .build(&maker)
    .unwrap();
    let spp = &transaction.spp_proof_inputs;

    // 1. The 1-in/2-out transact of the spec: the source in, two outputs.
    assert_eq!(spp.check_shape().unwrap(), Shape::IN1_OUT2);

    // 2. The change to the maker comes first, then the order UTXO.
    let change_hash = spr_test_utils::utxo::wallet(
        utxo(SOURCE - LOCKED, transaction.change_blinding),
        &maker.nullifier_key,
        TREE_ID,
        0,
        None,
        None,
    )
    .unwrap()
    .utxo_hash;
    let order_hash = transaction.order_utxo.hash(TREE_ID).unwrap();
    let output_hashes: Vec<[u8; 32]> = spp
        .external_data
        .outputs
        .iter()
        .map(|output| output.utxo_hash)
        .collect();
    assert_eq!((CHANGE_SLOT, ORDER_SLOT), (0, 1));
    assert_eq!(output_hashes, vec![change_hash, order_hash]);

    // 3. Exactly one message: the marker of the order output and the maker,
    // tagged to the taker.
    let marker = MessageData {
        view_tag: taker_address
            .signing_pubkey
            .confidential_view_tag()
            .unwrap(),
        data: borsh::to_vec(&MarkerData {
            order_utxo_hash: order_hash,
            maker_pubkey: maker_pubkey.to_bytes(),
        })
        .unwrap(),
    };
    assert_eq!(spp.external_data.messages, vec![marker]);
}
