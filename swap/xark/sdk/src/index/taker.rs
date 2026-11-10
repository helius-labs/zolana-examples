use std::time::Duration;

use anyhow::{bail, Result};
use borsh::BorshDeserialize;
use solana_address::Address;
use spr_client::{user_registry::try_resolve_registered_address, ClientError, Rpc};
use spr_keypair::{ShieldedAddress, ShieldedKeypair};
use spr_program::DataUtxo;
use spr_transaction::{
    serialization::confidential::Confidential, AssetRegistry, DecodeCx, ShieldedTransaction,
    UtxoSerialization,
};
use spr_user_registry_interface::{user_record_pda, user_registry_program_id};
use swap_program::circuits;

use super::{
    poll::{collect_tagged, index_until},
    scan::{decode_order, resolve_mint, unified_slots},
};
use crate::{
    err,
    shared::INDEXED_TREE_ID,
    state::{check_taker_pk_fe, OrderUtxo},
    MarkerData,
};

#[derive(Debug)]
pub struct TakerOrder {
    pub order_utxo: OrderUtxo,
    pub maker_pubkey: Address,
}

/// An order decrypted from a make transaction, before the maker it names is
/// checked against the user registry.
pub struct TakerOrderCandidate {
    /// The decoded order UTXO, whose state the SDK checked against the
    /// committed leaf.
    pub order: DataUtxo<circuits::OrderTerms>,
    pub maker_pubkey: Address,
    pub order_utxo_hash: [u8; 32],
}

/// The order the marker tagged to `keypair`'s taker names in `tx`, or `None`
/// when `tx` has no such marker or does not decode as an order for it.
///
/// Rule: an attacker-controlled field never aborts the scan. Anyone can post a
/// marker and an order ciphertext under the taker's view tag, so everything
/// read before the order UTXO hash is checked against the committed leaf,
/// including the asset id resolved in `registry`, skips the transaction when
/// it does not decode. Only errors an attacker cannot produce propagate.
pub fn scan_taker(
    tx: &ShieldedTransaction,
    keypair: &ShieldedKeypair,
    registry: &AssetRegistry,
) -> Result<Option<TakerOrderCandidate>> {
    let taker_tag = keypair
        .signing_pubkey()
        .confidential_view_tag()
        .map_err(err)?;
    Ok(tx
        .messages
        .iter()
        .find(|message| message.view_tag == taker_tag)
        .and_then(|marker_message| decode_candidate(tx, keypair, &marker_message.data, registry)))
}

/// Decodes the order `marker_data` names in `tx`, or `None` when any of it,
/// all attacker-controlled, does not decode.
fn decode_candidate(
    tx: &ShieldedTransaction,
    keypair: &ShieldedKeypair,
    marker_data: &[u8],
    registry: &AssetRegistry,
) -> Option<TakerOrderCandidate> {
    let marker = MarkerData::try_from_slice(marker_data).ok()?;
    let (slot_index, slot, body) = unified_slots(tx)
        .find(|(_, slot, _)| slot.output_context.hash == marker.order_utxo_hash)?;
    let cx = DecodeCx::for_slot(&keypair.viewing_key, tx, slot_index);
    let plaintext = Confidential::decode(&body, &cx).ok()?;
    let asset = resolve_mint(registry, plaintext.asset_id).ok()?;
    // The client writes the maker signer into the marker and the program
    // checks it; the order UTXO is owned by that maker's order authority.
    let maker_pubkey = Address::new_from_array(marker.maker_pubkey);
    let order = decode_order(tx, slot, slot_index, plaintext, asset, &maker_pubkey).ok()?;
    // The circuit reads a non-canonical `taker_pk_fe` as another taker, so the
    // order is refused before its maker is resolved in the user registry.
    check_taker_pk_fe(&order.data.taker_pk_fe).ok()?;
    Some(TakerOrderCandidate {
        order,
        maker_pubkey,
        order_utxo_hash: marker.order_utxo_hash,
    })
}

impl TakerOrderCandidate {
    /// Accepts the order if the maker it names is `registered_maker`, the
    /// address the user registry holds for the marker's maker pubkey, and the
    /// opening recomputes the order UTXO hash the marker names.
    pub fn into_order(
        self,
        registered_maker: ShieldedAddress,
        registry: &AssetRegistry,
    ) -> Result<TakerOrder> {
        let order_utxo = OrderUtxo::from_data_utxo(self.order, registered_maker, registry)?;
        if order_utxo.hash(INDEXED_TREE_ID)? != self.order_utxo_hash {
            bail!("reconstructed order utxo hash does not match the committed leaf");
        }
        Ok(TakerOrder {
            order_utxo,
            maker_pubkey: self.maker_pubkey,
        })
    }
}

pub fn index_taker<I: Rpc, R: Rpc>(
    keypair: &ShieldedKeypair,
    registry: &AssetRegistry,
    indexer: &I,
    rpc: &R,
    timeout: Duration,
) -> Result<Vec<TakerOrder>> {
    // The confidential view tag of the taker's signing key, under which its
    // order outputs are indexed.
    let owner_tag = keypair
        .signing_pubkey()
        .confidential_view_tag()
        .map_err(err)?;
    index_until(timeout, "taker orders", || {
        collect_tagged(owner_tag, indexer, |tx| {
            let Some(candidate) = scan_taker(tx, keypair, registry)? else {
                return Ok(None);
            };
            // A maker missing from the registry skips the transaction; RPC
            // errors propagate.
            let Some(maker) = resolve_maker(rpc, candidate.maker_pubkey)? else {
                return Ok(None);
            };
            // Same rule as `scan_taker`: an order naming another maker, an
            // opening off the committed leaf or a destination mint missing
            // from `registry` are all attacker-controlled and skip.
            Ok(candidate.into_order(maker, registry).ok())
        })
    })
}

/// The address the user registry holds for `maker_pubkey`, or `None` when it
/// holds no record or a record whose keys do not form an address. Both are
/// attacker-controlled and skip the order: the registry stores the viewing
/// pubkey unchecked, so anyone can register a bad one, and anyone can fund the
/// record PDA of an unregistered pubkey, leaving a System-owned account there,
/// then post an order naming it. RPC errors and a registry-owned record that
/// does not decode propagate.
fn resolve_maker<R: Rpc>(rpc: &R, maker_pubkey: Address) -> Result<Option<ShieldedAddress>> {
    let (record_pda, _) = user_record_pda(&maker_pubkey);
    let is_registry_record = rpc
        .get_account(record_pda)
        .map_err(err)?
        .is_some_and(|record| record.owner == user_registry_program_id());
    if !is_registry_record {
        return Ok(None);
    }
    match try_resolve_registered_address(rpc, maker_pubkey) {
        Ok(resolved) => Ok(resolved.map(|resolved| resolved.address)),
        Err(ClientError::AddressResolution(_)) => Ok(None),
        Err(error) => Err(err(error)),
    }
}

#[cfg(test)]
mod tests {
    use solana_account::Account;
    use solana_keypair::Keypair;
    use spr_user_registry_interface::{MergeEncryptionScheme, UserRecord};

    use spr_interface::event::OutputDataEncoding;
    use spr_keypair::ViewingKey;
    use spr_program::{circuit::DataHash, EncryptionCodec};
    use spr_transaction::{
        serialization::confidential::ConfidentialEncode, Data, DataRecord, EncryptedScheme, Mint,
    };

    use super::*;
    use crate::index::fixture::{order_fixture, order_fixture_with_destination, OrderFixture};

    #[test]
    fn scan_taker_reconstructs_terms_from_the_transaction() {
        let fixture = order_fixture();
        let candidate = scan_taker(&fixture.tx, &fixture.taker_keypair, &fixture.registry)
            .expect("scan")
            .expect("order candidate");
        let order = candidate
            .into_order(fixture.maker_address, &fixture.registry)
            .expect("order");
        assert_eq!(
            (order.order_utxo, order.maker_pubkey),
            (fixture.order_utxo, fixture.maker_pubkey)
        );
    }

    #[test]
    fn into_order_rejects_a_wrong_maker_address() {
        let fixture = order_fixture();
        let candidate = scan_taker(&fixture.tx, &fixture.taker_keypair, &fixture.registry)
            .expect("scan")
            .expect("order candidate");
        let taker_address = fixture
            .taker_keypair
            .shielded_address()
            .expect("taker address");
        let error = candidate
            .into_order(taker_address, &fixture.registry)
            .expect_err("a maker other than the registered one must fail");
        assert!(error.to_string().contains("names another maker"));
    }

    #[test]
    fn into_order_rejects_a_wrong_order_utxo_hash() {
        let fixture = order_fixture();
        let mut candidate = scan_taker(&fixture.tx, &fixture.taker_keypair, &fixture.registry)
            .expect("scan")
            .expect("order candidate");
        candidate.order.utxo.utxo.amount += 1;
        let error = candidate
            .into_order(fixture.maker_address, &fixture.registry)
            .expect_err("another opening must fail the hash check");
        assert!(error
            .to_string()
            .contains("does not match the committed leaf"));
    }

    /// The fixture with the data of the taker's marker message replaced.
    fn fixture_with_marker(data: impl FnOnce(&[u8]) -> Vec<u8>) -> OrderFixture {
        let mut fixture = order_fixture();
        let taker_tag = fixture
            .taker_keypair
            .signing_pubkey()
            .confidential_view_tag()
            .expect("taker tag");
        let marker = fixture
            .tx
            .messages
            .iter_mut()
            .find(|message| message.view_tag == taker_tag)
            .expect("taker marker");
        marker.data = data(&marker.data);
        fixture
    }

    #[test]
    fn scan_taker_skips_a_malformed_marker() {
        let fixture = fixture_with_marker(|data| data.iter().take(10).copied().collect());
        assert!(
            scan_taker(&fixture.tx, &fixture.taker_keypair, &fixture.registry)
                .expect("a malformed marker must not fail the scan")
                .is_none()
        );
    }

    #[test]
    fn scan_taker_skips_a_marker_without_its_order_output() {
        let fixture = fixture_with_marker(|data| {
            let mut marker = MarkerData::try_from_slice(data).expect("marker");
            marker.order_utxo_hash = [1u8; 32];
            borsh::to_vec(&marker).expect("marker bytes")
        });
        assert!(
            scan_taker(&fixture.tx, &fixture.taker_keypair, &fixture.registry)
                .expect("a marker naming no order output must not fail the scan")
                .is_none()
        );
    }

    #[test]
    fn scan_taker_skips_an_order_it_cannot_decrypt() {
        // A marker tagged to another key over an order encrypted to the taker.
        let fixture = order_fixture();
        let other_keypair = ShieldedKeypair::from_keypair(&Keypair::new_from_array([21u8; 32]))
            .expect("other keypair");
        let mut tx = fixture.tx;
        let other_tag = other_keypair
            .signing_pubkey()
            .confidential_view_tag()
            .expect("other tag");
        for message in tx.messages.iter_mut() {
            message.view_tag = other_tag;
        }
        assert!(scan_taker(&tx, &other_keypair, &fixture.registry)
            .expect("an undecryptable order must not fail the scan")
            .is_none());
    }

    #[test]
    fn scan_taker_ignores_transactions_for_other_takers() {
        let fixture = order_fixture();
        let other_keypair = ShieldedKeypair::from_keypair(&Keypair::new_from_array([21u8; 32]))
            .expect("other keypair");
        assert!(scan_taker(&fixture.tx, &other_keypair, &fixture.registry)
            .expect("scan")
            .is_none());
    }

    #[test]
    fn scan_taker_skips_an_order_with_an_unknown_asset() {
        let fixture = order_fixture();
        assert!(scan_taker(
            &fixture.tx,
            &fixture.taker_keypair,
            &AssetRegistry::default(),
        )
        .expect("an unknown asset id must not fail the scan")
        .is_none());
    }

    #[test]
    fn into_order_rejects_an_unknown_destination_asset() {
        // The scan resolves only the source asset; `into_order` resolves the
        // destination, and `index_taker` skips the order when it fails.
        let destination_mint = Address::new_from_array([4u8; 32]);
        let fixture = order_fixture_with_destination(Mint::new(destination_mint, 3));
        let scan = |registry: &AssetRegistry| {
            scan_taker(&fixture.tx, &fixture.taker_keypair, registry)
                .expect("an unknown destination asset must not fail the scan")
                .expect("order candidate")
        };
        assert!(scan(&fixture.registry)
            .into_order(fixture.maker_address, &fixture.registry)
            .is_err());
        let mut registry = fixture.registry.clone();
        registry
            .insert(3, destination_mint)
            .expect("register destination");
        let order = scan(&registry)
            .into_order(fixture.maker_address, &registry)
            .expect("order");
        assert_eq!(order.order_utxo, fixture.order_utxo);
    }

    /// The fixture with the state of its order UTXO rewritten by `edit`. The
    /// order is re-encrypted to the taker under a fresh transaction viewing
    /// key and committed under the hash of the new state, which the marker
    /// names, as anyone posting an order to the taker could.
    fn fixture_with_order_state(edit: impl FnOnce(&mut circuits::OrderTerms)) -> OrderFixture {
        let mut fixture = order_fixture();
        let taker_tag = fixture
            .taker_keypair
            .signing_pubkey()
            .confidential_view_tag()
            .expect("taker tag");
        let marker_message = fixture
            .tx
            .messages
            .iter()
            .find(|message| message.view_tag == taker_tag)
            .expect("taker marker");
        let mut marker = MarkerData::try_from_slice(&marker_message.data).expect("marker");
        let (slot_index, slot, body) = unified_slots(&fixture.tx)
            .find(|(_, slot, _)| slot.output_context.hash == marker.order_utxo_hash)
            .map(|(slot_index, slot, body)| (slot_index, slot.clone(), body))
            .expect("order slot");
        let cx = DecodeCx::for_slot(&fixture.taker_keypair.viewing_key, &fixture.tx, slot_index);
        let mut plaintext = Confidential::decode(&body, &cx).expect("order plaintext");
        let asset = resolve_mint(&fixture.registry, plaintext.asset_id).expect("source mint");
        let mut order = decode_order(
            &fixture.tx,
            &slot,
            slot_index,
            plaintext.clone(),
            asset,
            &fixture.maker_pubkey,
        )
        .expect("order");
        edit(&mut order.data);

        let state_bytes = order.data.serialize_for_encryption().expect("state bytes");
        plaintext.data = Data::new(
            plaintext
                .data
                .records
                .into_iter()
                .map(|record| match record {
                    DataRecord::UtxoData(_) => DataRecord::UtxoData(state_bytes.clone()),
                    other => other,
                })
                .collect(),
        );
        let state_hash = DataHash::hash(&order.data).expect("state hash");
        let data_hash =
            spr_program::hasher::data_hash(&spr_program::to_bytes(&state_hash)).expect("data hash");
        let order_utxo_hash = order
            .utxo
            .utxo
            .hash(
                &order.utxo.nullifier_pubkey,
                &data_hash,
                &[0u8; 32],
                slot.output_context.tree_id,
            )
            .expect("order utxo hash");

        let tx_viewing_key = ViewingKey::new();
        let tx_viewing_pk = tx_viewing_key.pubkey();
        let encode = ConfidentialEncode {
            tx: tx_viewing_key,
            recipient_pubkey: fixture.taker_keypair.viewing_pubkey(),
            salt: fixture.tx.salt.expect("salt"),
            slot_index,
        };
        let plaintext_bytes = plaintext.serialize().expect("plaintext bytes");
        let mut blob = vec![EncryptedScheme::Confidential.as_byte()];
        blob.extend(Confidential::encrypt(&plaintext_bytes, &encode).expect("ciphertext"));
        let order_slot = fixture
            .tx
            .output_slots
            .iter_mut()
            .find(|order_slot| order_slot.output_context.hash == marker.order_utxo_hash)
            .expect("order slot");
        order_slot.payload =
            borsh::to_vec(&OutputDataEncoding::Encrypted(blob)).expect("order payload");
        order_slot.output_context.hash = order_utxo_hash;
        fixture.tx.tx_viewing_pk = Some(tx_viewing_pk);

        marker.order_utxo_hash = order_utxo_hash;
        let marker_message = fixture
            .tx
            .messages
            .iter_mut()
            .find(|message| message.view_tag == taker_tag)
            .expect("taker marker");
        marker_message.data = borsh::to_vec(&marker).expect("marker bytes");
        fixture
    }

    #[test]
    fn scan_taker_skips_a_non_canonical_taker_pk_fe() {
        let scan = |fixture: &OrderFixture| {
            scan_taker(&fixture.tx, &fixture.taker_keypair, &fixture.registry)
                .expect("a forged order must not fail the scan")
        };
        // 1. A forged open order decodes, so the forgery itself is sound.
        let open = fixture_with_order_state(|state| state.taker_pk_fe = [0u8; 32]);
        let candidate = scan(&open).expect("open order candidate");
        assert_eq!(candidate.order.data.taker_pk_fe, [0u8; 32]);
        // 2. The same order with a taker at or above the BN254 modulus skips.
        let non_canonical = fixture_with_order_state(|state| state.taker_pk_fe = [0xffu8; 32]);
        assert!(scan(&non_canonical).is_none());
    }

    /// Serves `account` at `address`, or fails every account read with
    /// `rpc_error`.
    struct AccountRpc {
        account: Option<(Address, Account)>,
        rpc_error: Option<&'static str>,
    }

    impl Rpc for AccountRpc {
        fn get_account(&self, address: Address) -> Result<Option<Account>, ClientError> {
            if let Some(message) = self.rpc_error {
                return Err(ClientError::Rpc(message.to_string()));
            }
            Ok(self
                .account
                .as_ref()
                .filter(|(expected, _)| *expected == address)
                .map(|(_, account)| account.clone()))
        }
    }

    /// An RPC serving the user record of `keypair` with `viewing_pubkey` in
    /// place of its own, owned by `owner_program`.
    fn record_rpc(
        keypair: &ShieldedKeypair,
        viewing_pubkey: [u8; 33],
        owner_program: Address,
    ) -> (Address, AccountRpc) {
        let owner = keypair
            .shielded_address()
            .and_then(|address| address.solana_address())
            .expect("owner");
        let (pda, bump) = user_record_pda(&owner);
        let record = UserRecord {
            owner,
            bump,
            nullifier_pubkey: keypair.nullifier_key.pubkey().expect("nullifier pubkey"),
            viewing_pubkey,
            merge_encryption: MergeEncryptionScheme::Disabled,
        };
        let mut data = vec![UserRecord::DISCRIMINATOR];
        data.extend(borsh::to_vec(&record).expect("record bytes"));
        let account = Account {
            lamports: 1,
            data,
            owner: owner_program,
            executable: false,
            rent_epoch: 0,
        };
        let rpc = AccountRpc {
            account: Some((pda, account)),
            rpc_error: None,
        };
        (owner, rpc)
    }

    #[test]
    fn resolve_maker_returns_the_registered_address() {
        let fixture = order_fixture();
        let viewing_pubkey = *fixture.maker_keypair.viewing_pubkey().as_bytes();
        let (owner, rpc) = record_rpc(
            &fixture.maker_keypair,
            viewing_pubkey,
            user_registry_program_id(),
        );
        assert_eq!(
            resolve_maker(&rpc, owner).expect("resolve"),
            Some(fixture.maker_address)
        );
    }

    #[test]
    fn resolve_maker_skips_an_unregistered_maker() {
        let rpc = AccountRpc {
            account: None,
            rpc_error: None,
        };
        assert_eq!(
            resolve_maker(&rpc, order_fixture().maker_pubkey).expect("resolve"),
            None
        );
    }

    #[test]
    fn resolve_maker_skips_a_registered_invalid_viewing_pubkey() {
        let fixture = order_fixture();
        let (owner, rpc) = record_rpc(
            &fixture.maker_keypair,
            [0u8; 33],
            user_registry_program_id(),
        );
        assert_eq!(
            resolve_maker(&rpc, owner).expect("a bad registered viewing key must not fail"),
            None
        );
    }

    #[test]
    fn resolve_maker_fails_on_an_rpc_error() {
        let rpc = AccountRpc {
            account: None,
            rpc_error: Some("connection refused"),
        };
        let error = resolve_maker(&rpc, order_fixture().maker_pubkey)
            .expect_err("an RPC error must fail the scan");
        assert!(error.to_string().contains("connection refused"));
    }

    #[test]
    fn resolve_maker_skips_a_funded_unregistered_record_pda() {
        // Anyone can fund the record PDA of an unregistered pubkey, which
        // leaves an empty System-owned account there.
        let maker_pubkey = order_fixture().maker_pubkey;
        let funded = Account {
            lamports: 1_000_000,
            data: Vec::new(),
            // The System program.
            owner: Address::default(),
            executable: false,
            rent_epoch: 0,
        };
        let rpc = AccountRpc {
            account: Some((user_record_pda(&maker_pubkey).0, funded)),
            rpc_error: None,
        };
        assert_eq!(
            resolve_maker(&rpc, maker_pubkey).expect("a funded record PDA must not fail"),
            None
        );
    }

    #[test]
    fn resolve_maker_fails_on_an_undecodable_record() {
        let fixture = order_fixture();
        let viewing_pubkey = *fixture.maker_keypair.viewing_pubkey().as_bytes();
        let (owner, mut rpc) = record_rpc(
            &fixture.maker_keypair,
            viewing_pubkey,
            user_registry_program_id(),
        );
        let discriminator = rpc
            .account
            .as_mut()
            .and_then(|(_, account)| account.data.first_mut())
            .expect("record discriminator");
        *discriminator = UserRecord::DISCRIMINATOR.wrapping_add(1);
        let error = resolve_maker(&rpc, owner).expect_err("a record decode error must fail");
        assert!(error
            .to_string()
            .contains("invalid user registry record account data"));
    }
}
