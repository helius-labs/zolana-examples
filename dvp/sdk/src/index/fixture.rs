use solana_address::Address;
use solana_keypair::Keypair;
use solana_signature::Signature;
use zolana_client::{MerkleContext, NonInclusionProof};
use zolana_keypair::{P256Pubkey, ShieldedKeypair};
use zolana_transaction::{
    instructions::transact::{OutputContext, OutputSlot, SppProofInputs},
    AssetRegistry, Data, Mint, ShieldedTransaction, Utxo, WalletUtxo, SOL_MINT,
};

use crate::{
    instructions::{
        create_dvp::{CreateDvpDerivation, CreateDvpTransactionParams},
        fund_dvp::FundDvpParams,
    },
    state::{Deposit, DvpUtxo, Leg, SwapDvp},
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

/// Pool asset id the fixture registers the sample delivery mint under.
const FIXTURE_DELIVERY_ASSET_ID: u64 = 2;
/// What the fixture funders send above each leg amount.
pub(crate) const FIXTURE_SURPLUS: u64 = 11;

pub(crate) struct DepositFixture {
    pub(crate) terms: SwapDvp,
    pub(crate) registry: AssetRegistry,
    pub(crate) authority: ShieldedKeypair,
    /// Seller (funds delivery) and buyer (funds payment).
    pub(crate) funders: [ShieldedKeypair; 2],
    /// One funding transaction per funder, in funder order.
    pub(crate) txs: [ShieldedTransaction; 2],
    /// The deposit each transaction makes, in funder order.
    pub(crate) deposits: [Deposit; 2],
}

/// A spendable note of `amount` of `mint` for `owner`, as a wallet sync
/// would hold it.
fn wallet_utxo(owner: &ShieldedKeypair, mint: Mint, amount: u64) -> WalletUtxo {
    let utxo = Utxo {
        owner: owner.signing_pubkey(),
        asset: mint,
        amount,
        blinding: zolana_keypair::random_blinding(),
        ring_program_id: None,
        data: Data::default(),
    };
    let nullifier_pubkey = owner.nullifier_key.pubkey().expect("nullifier pubkey");
    let utxo_hash = utxo
        .hash(&nullifier_pubkey, &[0; 32], &[0; 32], FIXTURE_TREE_ID)
        .expect("utxo hash");
    let nullifier = owner
        .nullifier_key
        .nullifier(&utxo_hash, &utxo.blinding)
        .expect("nullifier");
    WalletUtxo {
        utxo,
        nullifier_pubkey,
        utxo_hash,
        nullifier,
        data_hash: None,
        ring_data_hash: None,
        tree_id: FIXTURE_TREE_ID,
        leaf_index: 0,
        slot: 0,
        tx_signature: Signature::default(),
        slot_index: 0,
    }
}

/// An encrypted transaction as the indexer serves it, its outputs appended
/// from leaf `first_leaf`.
fn shielded_transaction(proof_inputs: &SppProofInputs, first_leaf: u64) -> ShieldedTransaction {
    let external = &proof_inputs.external_data;
    let output_slots = external
        .outputs
        .iter()
        .zip(external.resolved_owner_tags.iter())
        .zip(first_leaf..)
        .map(|((output, view_tag), leaf_index)| OutputSlot {
            view_tag: *view_tag,
            output_context: OutputContext {
                hash: output.utxo_hash,
                tree_id: FIXTURE_TREE_ID,
                leaf_index,
            },
            payload: output.data.clone().unwrap_or_default(),
        })
        .collect();
    ShieldedTransaction {
        slot: 0,
        tx_signature: Signature::default(),
        event_index: Some(0),
        tx_viewing_pk: P256Pubkey::from_bytes(external.tx_viewing_pk).ok(),
        salt: Some(external.salt),
        output_slots,
        messages: external.messages.clone(),
        nullifiers: proof_inputs
            .input_utxos
            .iter()
            .map(|input| input.nullifier)
            .collect(),
        proofless: false,
        ring_config: None,
        ring_program_id: None,
    }
}

/// The seller funds the delivery leg and the buyer the payment leg, each
/// with [`FIXTURE_SURPLUS`] on top, through the sdk's own funding builder.
pub(crate) fn deposit_fixture() -> DepositFixture {
    let parties = fixture_parties();
    let terms = sample_terms(&parties);
    let [seller, buyer, authority] = parties;
    let mut registry = AssetRegistry::default();
    registry
        .insert(FIXTURE_DELIVERY_ASSET_ID, terms.delivery_mint)
        .expect("register delivery mint");
    let delivery_mint = Mint::new(terms.delivery_mint, FIXTURE_DELIVERY_ASSET_ID);
    let fund = |funder: &ShieldedKeypair, leg: Leg, mint: Mint, first_leaf: u64| {
        let amount = leg.amount(&terms) + FIXTURE_SURPLUS;
        let proof_inputs = FundDvpParams {
            terms: terms.clone(),
            leg,
            amount,
            authority_viewing_pubkey: authority.viewing_pubkey(),
            inputs: vec![wallet_utxo(funder, mint, 10 * amount)],
            payer: pubkey(funder),
            tree_id: FIXTURE_TREE_ID,
        }
        .build(funder)
        .expect("funding transaction");
        let tx = shielded_transaction(&proof_inputs, first_leaf);
        let (deposit_output, deposit_slot) = proof_inputs
            .output_utxos
            .iter()
            .zip(tx.output_slots.iter())
            .find(|(output, _)| output.asset == mint && output.amount == amount)
            .expect("deposit output");
        let deposit = Deposit {
            terms: terms.clone(),
            leg,
            mint,
            amount,
            blinding: deposit_output.blinding,
            tree_id: FIXTURE_TREE_ID,
            leaf_index: deposit_slot.output_context.leaf_index,
        };
        (tx, deposit)
    };
    let (delivery_tx, delivery) = fund(&seller, Leg::Delivery, delivery_mint, 10);
    let (payment_tx, payment) = fund(&buyer, Leg::Payment, Mint::SOL, 20);
    DepositFixture {
        terms,
        registry,
        authority,
        funders: [seller, buyer],
        txs: [delivery_tx, payment_tx],
        deposits: [delivery, payment],
    }
}
