use solana_address::Address;
use solana_keypair::Keypair;
use solana_signature::Signature;
use zolana_client::{MerkleContext, NonInclusionProof};
use zolana_keypair::{P256Pubkey, ShieldedKeypair};
use zolana_transaction::{
    instructions::transact::{OutputContext, OutputSlot},
    ShieldedTransaction, SOL_MINT,
};

use crate::{
    instructions::create_dvp::{CreateDvpDerivation, CreateDvpTransactionParams},
    state::{DvpUtxo, SwapDvp},
};

/// Tree the fixture's DvP UTXO is appended to (D6 pins the create to 0).
pub(crate) const FIXTURE_TREE_ID: u16 = 0;
/// Leaf index the fixture's DvP UTXO lands at.
pub(crate) const FIXTURE_LEAF_INDEX: u64 = 5;

pub(crate) struct TermsFixture {
    pub(crate) tx: ShieldedTransaction,
    /// Seller, buyer and settlement authority, in message order.
    pub(crate) parties: [ShieldedKeypair; 3],
    pub(crate) dvp_utxo: DvpUtxo,
}

pub(crate) fn keypair(seed: u8) -> ShieldedKeypair {
    ShieldedKeypair::from_keypair(&Keypair::new_from_array([seed; 32])).expect("keypair")
}

fn pubkey(keypair: &ShieldedKeypair) -> Address {
    keypair
        .shielded_address()
        .expect("shielded address")
        .solana_address()
        .expect("solana address")
}

pub(crate) fn sample_terms(parties: &[ShieldedKeypair; 3]) -> SwapDvp {
    let [seller, buyer, authority] = parties;
    let seller_address = seller.shielded_address().expect("seller address");
    let buyer_address = buyer.shielded_address().expect("buyer address");
    SwapDvp {
        seller: pubkey(seller),
        buyer: pubkey(buyer),
        delivery_mint: Address::new_from_array([3; 32]),
        payment_mint: SOL_MINT,
        settlement_authority: pubkey(authority),
        delivery_amount: 1_000,
        payment_amount: 2_500,
        expiry_timestamp: 1_800_000_000,
        nonce: 7,
        ref_string: [0; 64],
        seller_settlement_destination: seller_address,
        buyer_settlement_destination: buyer_address,
        earliest_settlement_timestamp: None,
        seller_refund_destination: seller_address,
        buyer_refund_destination: buyer_address,
        creator: pubkey(seller),
        blinding: zolana_keypair::random_blinding(),
    }
}

pub(crate) fn fixture_parties() -> [ShieldedKeypair; 3] {
    [keypair(7), keypair(13), keypair(21)]
}

/// Create transaction params for `terms` with a placeholder non-inclusion
/// proof of the DvP address (assembly reads only its leaf and roots).
pub(crate) fn create_params(
    terms: &SwapDvp,
    parties: &[ShieldedKeypair; 3],
) -> CreateDvpTransactionParams {
    let address = CreateDvpDerivation::new(terms, FIXTURE_TREE_ID)
        .expect("derivation")
        .address;
    CreateDvpTransactionParams {
        terms: terms.clone(),
        payer: terms.creator,
        recipients: parties
            .each_ref()
            .map(|party| party.shielded_address().expect("recipient address")),
        tree_id: FIXTURE_TREE_ID,
        non_inclusion: NonInclusionProof {
            leaf: address,
            merkle_context: MerkleContext {
                tree_type: 0,
                tree: Address::default(),
            },
            path: Vec::new(),
            low_element: [0; 32],
            low_element_index: 0,
            high_element: [0; 32],
            high_element_index: 0,
            root: [0; 32],
            root_seq: 0,
            root_index: 0,
        },
    }
}

/// A create transaction as the indexer serves it, built by the sdk's own
/// assembly.
pub(crate) fn terms_fixture() -> TermsFixture {
    let parties = fixture_parties();
    let terms = sample_terms(&parties);
    let transaction = create_params(&terms, &parties)
        .build()
        .expect("create transaction");
    let address = transaction.derivation.address;
    let spp = transaction.spp;
    let pda_tag = transaction.derivation.dvp_authority.to_bytes();
    let output_slots = spp
        .outputs
        .iter()
        .map(|output| OutputSlot {
            view_tag: pda_tag,
            output_context: OutputContext {
                hash: output.utxo_hash,
                tree_id: FIXTURE_TREE_ID,
                leaf_index: FIXTURE_LEAF_INDEX,
            },
            payload: Vec::new(),
        })
        .collect();
    let tx = ShieldedTransaction {
        slot: 0,
        tx_signature: Signature::default(),
        event_index: Some(0),
        tx_viewing_pk: P256Pubkey::from_bytes(spp.tx_viewing_pk).ok(),
        salt: Some(spp.salt),
        output_slots,
        messages: spp.messages,
        nullifiers: vec![address],
        proofless: false,
        ring_config: None,
        ring_program_id: None,
    };
    TermsFixture {
        tx,
        parties,
        dvp_utxo: DvpUtxo::new(terms, FIXTURE_TREE_ID),
    }
}
