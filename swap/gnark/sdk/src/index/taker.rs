use std::time::Duration;

use anyhow::{bail, Result};
use borsh::BorshDeserialize;
use solana_address::Address;
use solana_pubkey::Pubkey;
use spr_client::{user_registry::try_resolve_registered_address, ClientError, Rpc};
use spr_keypair::{ShieldedAddress, ShieldedKeypair, X25519Pubkey};
use spr_transaction::{
    serialization::confidential::Confidential, utxo::Blinding, AssetRegistry, DecodeCx,
    ShieldedTransaction, UtxoSerialization,
};

use super::{
    poll::{collect_tagged, index_until},
    scan::{parse_order_data, resolve_mint, unified_slots},
};
use crate::{
    err,
    shared::INDEXED_TREE_ID,
    state::{OrderTerms, OrderUtxo, PlainTextData},
    MarkerData,
};

#[derive(Debug)]
pub struct TakerOrder {
    pub order_utxo: OrderUtxo,
    pub maker_pubkey: Pubkey,
}

pub struct TakerOrderCandidate {
    pub source_amount: u64,
    pub source_mint: spr_transaction::Mint,
    pub destination_mint: Address,
    pub order_utxo_blinding: Blinding,
    pub order_data: PlainTextData,
    pub maker_pubkey: Pubkey,
    pub order_utxo_hash: [u8; 32],
}

/// The order the marker tagged to `keypair`'s taker names in `tx`, or `None`
/// when `tx` has no such marker or does not decode as an order for it.
///
/// Rule: an attacker-controlled field never aborts the scan. Anyone can post a
/// marker and an order ciphertext under the taker's view tag, so everything
/// read before the order UTXO hash is checked against the committed leaf,
/// including the asset ids resolved in `registry`, skips the transaction when
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
    let (slot_index, _, body) = unified_slots(tx)
        .find(|(_, slot, _)| slot.output_context.hash == marker.order_utxo_hash)?;
    let cx = DecodeCx::for_slot(&keypair.viewing_key, tx, slot_index);
    let order_utxo_plaintext = Confidential::decode(&body, &cx).ok()?;
    let order_data = parse_order_data(&order_utxo_plaintext.data.records).ok()?;
    Some(TakerOrderCandidate {
        source_amount: order_utxo_plaintext.amount,
        source_mint: resolve_mint(registry, order_utxo_plaintext.asset_id).ok()?,
        destination_mint: resolve_mint(registry, order_data.destination_asset_id)
            .ok()?
            .asset,
        order_utxo_blinding: order_utxo_plaintext.blinding,
        order_data,
        maker_pubkey: Pubkey::new_from_array(marker.maker_pubkey),
        order_utxo_hash: marker.order_utxo_hash,
    })
}

impl TakerOrderCandidate {
    pub fn into_order(
        self,
        destination: ShieldedAddress,
        taker_viewing_pubkey: X25519Pubkey,
    ) -> Result<TakerOrder> {
        let terms = OrderTerms {
            destination_mint: self.destination_mint,
            destination_amount: self.order_data.destination_amount,
            destination,
            taker: self.order_data.taker,
            expiry: self.order_data.expiry,
        };
        let order_utxo = OrderUtxo {
            terms,
            blinding: self.order_utxo_blinding,
            source_mint: self.source_mint,
            source_amount: self.source_amount,
            destination_asset_id: self.order_data.destination_asset_id,
        };
        let order_utxo_hash = order_utxo
            .output_utxo(taker_viewing_pubkey)?
            .hash(INDEXED_TREE_ID)
            .map_err(err)?;
        if order_utxo_hash != self.order_utxo_hash {
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
    let taker_viewing_pubkey = keypair.viewing_pubkey();
    index_until(timeout, "taker orders", || {
        collect_tagged(keypair, indexer, |tx| {
            let Some(candidate) = scan_taker(tx, keypair, registry)? else {
                return Ok(None);
            };
            // A maker missing from the registry skips the transaction; RPC
            // errors propagate. Same rule as `scan_taker`: an order that names
            // another maker fails the hash check and skips.
            let Some(maker_address) = resolve_maker(rpc, candidate.maker_pubkey)? else {
                return Ok(None);
            };
            Ok(candidate
                .into_order(maker_address, taker_viewing_pubkey)
                .ok())
        })
    })
}

/// The address the user registry holds for `maker_pubkey`, or `None` when it
/// holds no record or a record whose keys do not form an address. The
/// registry stores the viewing pubkey unchecked, so anyone can register a bad
/// one and post an order naming it; that skips the order. RPC and record
/// decode errors propagate: the record PDA is the registry program's.
fn resolve_maker<R: Rpc>(rpc: &R, maker_pubkey: Pubkey) -> Result<Option<ShieldedAddress>> {
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
    use spr_user_registry_interface::{
        user_record_pda, user_registry_program_id, MergeEncryptionScheme, UserRecord,
    };

    use super::*;
    use crate::index::fixture::{order_fixture, OrderFixture};

    #[test]
    fn scan_taker_reconstructs_terms_from_the_transaction() {
        let fixture = order_fixture();
        let candidate = scan_taker(&fixture.tx, &fixture.taker_keypair, &fixture.registry)
            .expect("scan")
            .expect("order candidate");
        let order = candidate
            .into_order(
                fixture.maker_address,
                fixture.taker_keypair.viewing_pubkey(),
            )
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
            .into_order(taker_address, fixture.taker_keypair.viewing_pubkey())
            .expect_err("wrong maker address must fail the hash check");
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

    /// Serves `account` at `address`, or fails every account read with
    /// `rpc_error`.
    struct AccountRpc {
        account: Option<(Pubkey, Account)>,
        rpc_error: Option<&'static str>,
    }

    impl Rpc for AccountRpc {
        fn get_account(&self, address: Pubkey) -> Result<Option<Account>, ClientError> {
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
        owner_program: Pubkey,
    ) -> (Pubkey, AccountRpc) {
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
    fn resolve_maker_fails_on_an_undecodable_record() {
        let fixture = order_fixture();
        let viewing_pubkey = *fixture.maker_keypair.viewing_pubkey().as_bytes();
        let (owner, rpc) = record_rpc(
            &fixture.maker_keypair,
            viewing_pubkey,
            Pubkey::new_from_array([5u8; 32]),
        );
        let error = resolve_maker(&rpc, owner).expect_err("a record decode error must fail");
        assert!(error.to_string().contains("not owned by the user registry"));
    }
}
