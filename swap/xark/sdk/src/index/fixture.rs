use solana_address::Address;
use solana_keypair::Keypair;
use solana_signature::Signature;
use spr_keypair::{ShieldedAddress, ShieldedKeypair, X25519Pubkey};
use spr_transaction::{
    instructions::transact::{OutputContext, OutputSlot, SppProofInputs, SppProofOutputUtxo},
    AssetRegistry, Mint, ShieldedTransaction,
};

use crate::{
    instructions::make::MakeTransactionParams,
    order_authority,
    shared::{identity, test_blinding, INDEXED_TREE_ID},
    state::{OrderTerms, OrderUtxo},
};

pub(crate) struct OrderFixture {
    pub(crate) tx: ShieldedTransaction,
    pub(crate) registry: AssetRegistry,
    pub(crate) taker_keypair: ShieldedKeypair,
    pub(crate) maker_keypair: ShieldedKeypair,
    pub(crate) order_utxo: OrderUtxo,
    pub(crate) maker_address: ShieldedAddress,
    pub(crate) maker_pubkey: Address,
}

/// The indexed form of a proved transaction: every output appended to the
/// indexed tree in slot order.
fn shielded_transaction(proof_inputs: &SppProofInputs) -> ShieldedTransaction {
    let external = &proof_inputs.external_data;
    let output_slots = external
        .outputs
        .iter()
        .zip(external.resolved_owner_tags.iter())
        .enumerate()
        .map(|(index, (output, view_tag))| OutputSlot {
            view_tag: *view_tag,
            output_context: OutputContext {
                hash: output.utxo_hash,
                tree_id: INDEXED_TREE_ID,
                leaf_index: index as u64,
            },
            payload: output.data.clone().unwrap_or_default(),
        })
        .collect();
    let nullifiers = proof_inputs
        .input_utxo_hashes()
        .expect("input commitments")
        .iter()
        .map(|commitment| commitment.nullifier)
        .collect();
    ShieldedTransaction {
        slot: 0,
        tx_signature: Signature::default(),
        event_index: Some(0),
        tx_viewing_pk: X25519Pubkey::from_bytes(external.tx_viewing_pk).ok(),
        salt: Some(external.salt),
        output_slots,
        messages: external.messages.clone(),
        nullifiers,
        proofless: false,
        ring_config: None,
        ring_program_id: None,
    }
}

/// A make of a 400 000-unit order, designated to the taker and encrypted to
/// it, from a 1 000 000-unit source owned by the order authority.
pub(crate) fn order_fixture() -> OrderFixture {
    let maker_keypair =
        ShieldedKeypair::from_keypair(&Keypair::new_from_array([7u8; 32])).expect("maker keypair");
    let taker_keypair =
        ShieldedKeypair::from_keypair(&Keypair::new_from_array([13u8; 32])).expect("taker keypair");
    let maker_address = maker_keypair.shielded_address().expect("maker address");
    let taker_address = taker_keypair.shielded_address().expect("taker address");
    let maker_pubkey = maker_address
        .solana_address()
        .expect("maker solana address");
    let source_mint = Address::new_from_array([9u8; 32]);
    let mut registry = AssetRegistry::default();
    registry.insert(2, source_mint).expect("register mint");
    let source_mint = registry.mint(&source_mint).expect("source mint");

    let source = order_authority()
        .input(
            &SppProofOutputUtxo {
                asset: source_mint,
                amount: 1_000_000,
                blinding: test_blinding(5),
                owner_address: Some(
                    order_authority()
                        .address(maker_address.viewing_pubkey)
                        .expect("order authority address"),
                ),
                ..Default::default()
            },
            INDEXED_TREE_ID,
            0,
        )
        .expect("source input");
    let terms = OrderTerms {
        destination_mint: Mint::SOL,
        destination_amount: 250_000,
        maker: maker_address,
        expiry: 2_000_000_000,
        taker: Some(
            identity(
                &taker_address
                    .solana_address()
                    .expect("taker solana address"),
            )
            .expect("taker identity"),
        ),
    };
    let transaction = MakeTransactionParams {
        terms,
        source_amount: 400_000,
        source,
        taker_address,
        payer: maker_pubkey,
        tree_id: INDEXED_TREE_ID,
    }
    .build(&maker_keypair)
    .expect("make transaction");

    OrderFixture {
        tx: shielded_transaction(&transaction.spp_proof_inputs),
        registry,
        taker_keypair,
        maker_keypair,
        order_utxo: transaction.order_utxo,
        maker_address,
        maker_pubkey,
    }
}
