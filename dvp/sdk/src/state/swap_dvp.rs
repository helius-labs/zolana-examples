use anyhow::{bail, ensure, Result};
use dvp_program::{
    instructions::shared::u64_right_align, ABORT_TAG, CREATE_TAG, DVP_AUTHORITY_PDA_SEED,
    SETTLEMENT_TAG,
};
use dvp_prover::{DestinationProofInput, SwapDvpProofInput};
use solana_address::Address;
use zolana_hasher::primitives::{hash_bytes, solana_owner_identity};
use zolana_keypair::{hash::poseidon, shielded::SHIELDED_ADDRESS_LEN, Curve, ShieldedAddress};
use zolana_transaction::instructions::transact::asset_field;

use crate::err;

/// Byte length of [`SwapDvp::to_bytes`].
pub const SWAP_DVP_LEN: usize = 5 * 32 + 4 * 8 + 64 + 4 * SHIELDED_ADDRESS_LEN + 1 + 8 + 2 * 32;

/// The DvP terms (spec "Terms"). Every party learns them from the encrypted
/// create messages; the program only ever sees `dvp_id`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SwapDvp {
    pub seller: Address,
    pub buyer: Address,
    pub delivery_mint: Address,
    pub payment_mint: Address,
    pub settlement_authority: Address,
    pub delivery_amount: u64,
    pub payment_amount: u64,
    /// Settlement fails after this time. Cluster time (`Clock::unix_timestamp`).
    pub expiry_timestamp: i64,
    /// Distinguishes DvPs with otherwise identical terms.
    pub nonce: u64,
    /// Client reference, UTF-8 zero-padded. The DvP program does not read it.
    pub ref_string: [u8; 64],
    /// Receives the payment leg at settlement.
    pub seller_settlement_destination: ShieldedAddress,
    /// Receives the delivery leg at settlement.
    pub buyer_settlement_destination: ShieldedAddress,
    /// Settlement fails before this time, if set.
    pub earliest_settlement_timestamp: Option<i64>,
    /// Receives the seller's delivery refunds: settle surplus, cancel, and reject.
    pub seller_refund_destination: ShieldedAddress,
    /// Receives the buyer's payment refunds: settle surplus, cancel, and reject.
    pub buyer_refund_destination: ShieldedAddress,
    /// Signs `create_dvp` and seeds the DvP PDA. Can be a single-use keypair.
    pub creator: Address,
    /// Random field element (`random_blinding()`). Without it, guessed terms
    /// can be checked against `dvp_id`.
    pub blinding: [u8; 32],
}

// create_dvp: the terms the creator commits to; every instruction recomputes
// `dvp_id` from them.
impl SwapDvp {
    /// Rejects terms no create, settle or abort proof can accept, before any
    /// proving work: the create checks, D5 (timestamps are non-negative) and
    /// D10 (settlement and refund destinations need an Ed25519 or PDA signing
    /// key, since the Eddsa rail binds every output owner to a Solana
    /// identity).
    pub fn validate(&self) -> Result<()> {
        ensure!(self.delivery_amount != 0, "delivery amount is zero");
        ensure!(self.payment_amount != 0, "payment amount is zero");
        ensure!(
            self.delivery_mint != self.payment_mint,
            "delivery and payment mints are equal"
        );
        ensure!(self.expiry_timestamp >= 0, "expiry timestamp is negative");
        if let Some(earliest) = self.earliest_settlement_timestamp {
            ensure!(earliest >= 0, "earliest settlement timestamp is negative");
            ensure!(
                earliest <= self.expiry_timestamp,
                "earliest settlement timestamp is after expiry"
            );
        }
        for destination in self.destinations() {
            match destination.signing_pubkey.curve().map_err(err)? {
                Curve::Ed25519 | Curve::Pda => {}
                Curve::P256 => bail!("destination has a P256 signing key (D10)"),
            }
        }
        Ok(())
    }

    /// The four destinations in `dvp_id` order.
    fn destinations(&self) -> [&ShieldedAddress; 4] {
        [
            &self.seller_settlement_destination,
            &self.buyer_settlement_destination,
            &self.seller_refund_destination,
            &self.buyer_refund_destination,
        ]
    }

    /// The DvP id (D4), nested because Poseidon takes at most 12 inputs:
    ///
    /// ```text
    /// dvp_id = Poseidon(parties_hash, legs_hash, schedule_hash,
    ///                   destinations_hash, hash_bytes(ref_string), blinding)
    /// ```
    ///
    /// See [`swap_dvp_id`] for the inner hashes.
    pub fn dvp_id(&self) -> Result<[u8; 32]> {
        swap_dvp_id(&SwapDvpProofInput::try_from(self)?)
    }

    /// The per-DvP authority PDA `[b"dvp", creator, dvp_id]` and its bump.
    pub fn dvp_authority(&self) -> Result<(Address, u8)> {
        let dvp_id = self.dvp_id()?;
        Ok(Address::find_program_address(
            &[DVP_AUTHORITY_PDA_SEED, self.creator.as_ref(), &dvp_id],
            &dvp_program::ID,
        ))
    }

    /// Seed of the create transaction's output blindings.
    pub fn create_blinding_seed(&self) -> Result<[u8; 32]> {
        self.blinding_seed(CREATE_TAG)
    }

    /// Seed of the settle transaction's output blindings.
    pub fn settle_blinding_seed(&self) -> Result<[u8; 32]> {
        self.blinding_seed(SETTLEMENT_TAG)
    }

    /// Seed of the cancel and reject transactions' output blindings.
    pub fn abort_blinding_seed(&self) -> Result<[u8; 32]> {
        self.blinding_seed(ABORT_TAG)
    }

    /// `Poseidon(tag, blinding)`, the tag right-aligned in a field element.
    fn blinding_seed(&self, tag: u32) -> Result<[u8; 32]> {
        poseidon(&[&u64_right_align(u64::from(tag)), &self.blinding]).map_err(err)
    }
}

/// `dvp_id` from the circuit-side terms (D4). The Go twin is
/// `dvpterms.SwapDvp.DvpID`.
///
/// ```text
/// parties_hash      = Poseidon(id(seller), id(buyer), id(settlement_authority), id(creator))
/// legs_hash         = Poseidon(asset(delivery_mint), asset(payment_mint), delivery_amount, payment_amount)
/// schedule_hash     = Poseidon(expiry_timestamp, has_earliest, earliest_settlement_timestamp, nonce)
/// destinations_hash = Poseidon(dest(seller_settlement), dest(buyer_settlement), dest(seller_refund), dest(buyer_refund))
/// ```
fn swap_dvp_id(input: &SwapDvpProofInput) -> Result<[u8; 32]> {
    let parties_hash = poseidon(&[
        &input.seller_id,
        &input.buyer_id,
        &input.settlement_authority_id,
        &input.creator_id,
    ])
    .map_err(err)?;
    let legs_hash = poseidon(&[
        &input.delivery_asset,
        &input.payment_asset,
        &u64_right_align(input.delivery_amount),
        &u64_right_align(input.payment_amount),
    ])
    .map_err(err)?;
    let schedule_hash = poseidon(&[
        &u64_right_align(input.expiry_timestamp),
        &u64_right_align(u64::from(input.has_earliest_settlement)),
        &u64_right_align(input.earliest_settlement_timestamp),
        &u64_right_align(input.nonce),
    ])
    .map_err(err)?;
    let destinations_hash = poseidon(&[
        &destination_hash(&input.seller_settlement_destination)?,
        &destination_hash(&input.buyer_settlement_destination)?,
        &destination_hash(&input.seller_refund_destination)?,
        &destination_hash(&input.buyer_refund_destination)?,
    ])
    .map_err(err)?;
    poseidon(&[
        &parties_hash,
        &legs_hash,
        &schedule_hash,
        &destinations_hash,
        &input.ref_string_hash,
        &input.blinding,
    ])
    .map_err(err)
}

/// `CompressedShieldedAddress::hash()`: `Poseidon(owner_hash, viewing_pk_hash)`.
fn destination_hash(destination: &DestinationProofInput) -> Result<[u8; 32]> {
    poseidon(&[&destination.owner_hash, &destination.viewing_pk_hash]).map_err(err)
}

fn destination_proof_input(address: &ShieldedAddress) -> Result<DestinationProofInput> {
    Ok(DestinationProofInput {
        owner_hash: address.owner_hash().map_err(err)?,
        viewing_pk_hash: hash_bytes(address.viewing_pubkey.as_bytes()).map_err(err)?,
    })
}

/// D5: timestamps enter the circuits as 63-bit values, so negative ones are
/// rejected here rather than wrapped.
fn timestamp(value: i64) -> Result<u64> {
    u64::try_from(value).map_err(|_| anyhow::anyhow!("timestamp {value} is negative (D5)"))
}

fn identity(address: &Address) -> Result<[u8; 32]> {
    solana_owner_identity(address.as_array()).map_err(err)
}

// All instructions: the terms enter the circuits in this form.
impl TryFrom<&SwapDvp> for SwapDvpProofInput {
    type Error = anyhow::Error;

    fn try_from(terms: &SwapDvp) -> Result<Self> {
        Ok(Self {
            seller_id: identity(&terms.seller)?,
            buyer_id: identity(&terms.buyer)?,
            settlement_authority_id: identity(&terms.settlement_authority)?,
            creator_id: identity(&terms.creator)?,
            delivery_asset: asset_field(&terms.delivery_mint).map_err(err)?,
            payment_asset: asset_field(&terms.payment_mint).map_err(err)?,
            delivery_amount: terms.delivery_amount,
            payment_amount: terms.payment_amount,
            expiry_timestamp: timestamp(terms.expiry_timestamp)?,
            has_earliest_settlement: terms.earliest_settlement_timestamp.is_some(),
            earliest_settlement_timestamp: terms
                .earliest_settlement_timestamp
                .map_or(Ok(0), timestamp)?,
            nonce: terms.nonce,
            ref_string_hash: hash_bytes(&terms.ref_string).map_err(err)?,
            seller_settlement_destination: destination_proof_input(
                &terms.seller_settlement_destination,
            )?,
            buyer_settlement_destination: destination_proof_input(
                &terms.buyer_settlement_destination,
            )?,
            seller_refund_destination: destination_proof_input(&terms.seller_refund_destination)?,
            buyer_refund_destination: destination_proof_input(&terms.buyer_refund_destination)?,
            blinding: terms.blinding,
        })
    }
}

// create_dvp: the plaintext of the terms messages; discovery decodes it.
impl SwapDvp {
    /// Fixed layout in field order: addresses as 32 bytes, integers
    /// little-endian, shielded addresses as their 99-byte `to_bytes`, and the
    /// optional earliest timestamp as a flag byte plus 8 bytes (0 when unset).
    pub fn to_bytes(&self) -> Vec<u8> {
        let earliest = self.earliest_settlement_timestamp;
        let mut bytes = Vec::with_capacity(SWAP_DVP_LEN);
        bytes.extend_from_slice(self.seller.as_ref());
        bytes.extend_from_slice(self.buyer.as_ref());
        bytes.extend_from_slice(self.delivery_mint.as_ref());
        bytes.extend_from_slice(self.payment_mint.as_ref());
        bytes.extend_from_slice(self.settlement_authority.as_ref());
        bytes.extend_from_slice(&self.delivery_amount.to_le_bytes());
        bytes.extend_from_slice(&self.payment_amount.to_le_bytes());
        bytes.extend_from_slice(&self.expiry_timestamp.to_le_bytes());
        bytes.extend_from_slice(&self.nonce.to_le_bytes());
        bytes.extend_from_slice(&self.ref_string);
        bytes.extend_from_slice(&self.seller_settlement_destination.to_bytes());
        bytes.extend_from_slice(&self.buyer_settlement_destination.to_bytes());
        bytes.push(u8::from(earliest.is_some()));
        bytes.extend_from_slice(&earliest.unwrap_or(0).to_le_bytes());
        bytes.extend_from_slice(&self.seller_refund_destination.to_bytes());
        bytes.extend_from_slice(&self.buyer_refund_destination.to_bytes());
        bytes.extend_from_slice(self.creator.as_ref());
        bytes.extend_from_slice(&self.blinding);
        bytes
    }

    /// Inverse of [`Self::to_bytes`]. Rejects a wrong length, an unknown flag
    /// byte, a nonzero timestamp behind an unset flag, and invalid shielded
    /// addresses.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        ensure!(
            bytes.len() == SWAP_DVP_LEN,
            "terms are {} bytes, expected {SWAP_DVP_LEN}",
            bytes.len()
        );
        let mut reader = Reader(bytes);
        let seller = Address::new_from_array(reader.take()?);
        let buyer = Address::new_from_array(reader.take()?);
        let delivery_mint = Address::new_from_array(reader.take()?);
        let payment_mint = Address::new_from_array(reader.take()?);
        let settlement_authority = Address::new_from_array(reader.take()?);
        let delivery_amount = u64::from_le_bytes(reader.take()?);
        let payment_amount = u64::from_le_bytes(reader.take()?);
        let expiry_timestamp = i64::from_le_bytes(reader.take()?);
        let nonce = u64::from_le_bytes(reader.take()?);
        let ref_string = reader.take()?;
        let seller_settlement_destination = reader.shielded_address()?;
        let buyer_settlement_destination = reader.shielded_address()?;
        let [has_earliest] = reader.take()?;
        let earliest = i64::from_le_bytes(reader.take()?);
        let earliest_settlement_timestamp = match has_earliest {
            0 if earliest == 0 => None,
            0 => bail!("earliest settlement timestamp set behind an unset flag"),
            1 => Some(earliest),
            flag => bail!("invalid earliest settlement flag {flag}"),
        };
        let seller_refund_destination = reader.shielded_address()?;
        let buyer_refund_destination = reader.shielded_address()?;
        let creator = Address::new_from_array(reader.take()?);
        let blinding = reader.take()?;
        Ok(Self {
            seller,
            buyer,
            delivery_mint,
            payment_mint,
            settlement_authority,
            delivery_amount,
            payment_amount,
            expiry_timestamp,
            nonce,
            ref_string,
            seller_settlement_destination,
            buyer_settlement_destination,
            earliest_settlement_timestamp,
            seller_refund_destination,
            buyer_refund_destination,
            creator,
            blinding,
        })
    }
}

struct Reader<'a>(&'a [u8]);

impl Reader<'_> {
    fn take<const N: usize>(&mut self) -> Result<[u8; N]> {
        let (head, rest) = self
            .0
            .split_first_chunk::<N>()
            .ok_or_else(|| anyhow::anyhow!("terms bytes too short"))?;
        self.0 = rest;
        Ok(*head)
    }

    fn shielded_address(&mut self) -> Result<ShieldedAddress> {
        ShieldedAddress::from_bytes(&self.take::<SHIELDED_ADDRESS_LEN>()?).map_err(err)
    }
}

#[cfg(test)]
mod tests {
    use zolana_gnark_ffi_prover::decimal;
    use zolana_keypair::{NullifierKey, PublicKey, ViewingKey};

    use super::*;

    /// The cross-language vector: the Go test `dvpterms_test.go` assigns these
    /// witness values and checks `DvpID` against `dvp_id`.
    const VECTOR_PATH: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../prover/circuits/dvpterms/testdata/swap_dvp_vector.json"
    );
    const VECTOR_DVP_ID: &str =
        "19284217639585534903980865803314903667065340572521528256377547822764170087020";

    fn destination(seed: u8) -> ShieldedAddress {
        ShieldedAddress {
            signing_pubkey: PublicKey::from_ed25519(&[seed; 32]),
            nullifier_pubkey: NullifierKey::from_secret([seed; 31]).pubkey().unwrap(),
            viewing_pubkey: ViewingKey::from_bytes(&[seed; 32]).unwrap().pubkey(),
        }
    }

    fn fixed_terms() -> SwapDvp {
        let mut ref_string = [0u8; 64];
        ref_string
            .iter_mut()
            .zip(b"dvp vector")
            .for_each(|(dst, src)| *dst = *src);
        let mut blinding = [0u8; 32];
        blinding
            .iter_mut()
            .skip(1)
            .enumerate()
            .for_each(|(i, byte)| *byte = i as u8 + 1);
        SwapDvp {
            seller: Address::new_from_array([1; 32]),
            buyer: Address::new_from_array([2; 32]),
            delivery_mint: Address::new_from_array([3; 32]),
            payment_mint: Address::new_from_array([0; 32]),
            settlement_authority: Address::new_from_array([4; 32]),
            delivery_amount: 1_000,
            payment_amount: 2_500_000_000,
            expiry_timestamp: 1_800_000_000,
            nonce: 7,
            ref_string,
            seller_settlement_destination: destination(11),
            buyer_settlement_destination: destination(12),
            earliest_settlement_timestamp: Some(1_700_000_000),
            seller_refund_destination: destination(13),
            buyer_refund_destination: destination(14),
            creator: Address::new_from_array([5; 32]),
            blinding,
        }
    }

    fn vector_json(terms: &SwapDvp) -> String {
        let entries = SwapDvpProofInput::try_from(terms)
            .unwrap()
            .witness_entries("Terms");
        let witness: Vec<String> = entries
            .iter()
            .map(|(key, values)| {
                let values: Vec<String> = values.iter().map(|v| format!("\"{v}\"")).collect();
                format!("    \"{key}\": [{}]", values.join(", "))
            })
            .collect();
        format!(
            "{{\n  \"dvp_id\": \"{}\",\n  \"create_blinding_seed\": \"{}\",\n  \"witness\": {{\n{}\n  }}\n}}\n",
            decimal(&terms.dvp_id().unwrap()),
            decimal(&terms.create_blinding_seed().unwrap()),
            witness.join(",\n")
        )
    }

    #[test]
    fn to_bytes_round_trip() {
        let terms = fixed_terms();
        let bytes = terms.to_bytes();
        assert_eq!(bytes.len(), SWAP_DVP_LEN);
        assert_eq!(SwapDvp::from_bytes(&bytes).unwrap(), terms);

        let unset = SwapDvp {
            earliest_settlement_timestamp: None,
            ..fixed_terms()
        };
        assert_eq!(SwapDvp::from_bytes(&unset.to_bytes()).unwrap(), unset);
    }

    #[test]
    fn from_bytes_rejects_malformed() {
        let bytes = fixed_terms().to_bytes();
        assert!(SwapDvp::from_bytes(bytes.split_last().unwrap().1).is_err());
        let mut longer = bytes.clone();
        longer.push(0);
        assert!(SwapDvp::from_bytes(&longer).is_err());

        let flag_offset = 5 * 32 + 4 * 8 + 64 + 2 * SHIELDED_ADDRESS_LEN;
        let mut bad_flag = bytes.clone();
        *bad_flag.get_mut(flag_offset).unwrap() = 2;
        assert!(SwapDvp::from_bytes(&bad_flag).is_err());
        let mut hidden_value = bytes;
        *hidden_value.get_mut(flag_offset).unwrap() = 0;
        assert!(SwapDvp::from_bytes(&hidden_value).is_err());
    }

    #[test]
    fn dvp_id_vector() {
        let terms = fixed_terms();
        let dvp_id = terms.dvp_id().unwrap();
        println!("dvp_id = {}", decimal(&dvp_id));
        assert_eq!(decimal(&dvp_id), VECTOR_DVP_ID);

        let json = vector_json(&terms);
        if std::env::var_os("DVP_UPDATE_VECTORS").is_some() {
            std::fs::write(VECTOR_PATH, &json).unwrap();
        }
        let committed = std::fs::read_to_string(VECTOR_PATH).unwrap();
        assert_eq!(committed, json, "rerun with DVP_UPDATE_VECTORS=1");
    }

    #[test]
    fn dvp_id_changes_with_every_field() {
        let base = fixed_terms();
        let base_id = base.dvp_id().unwrap();
        let mutations: [fn(&mut SwapDvp); 17] = [
            |t| t.seller = Address::new_from_array([9; 32]),
            |t| t.buyer = Address::new_from_array([9; 32]),
            |t| t.delivery_mint = Address::new_from_array([9; 32]),
            |t| t.payment_mint = Address::new_from_array([9; 32]),
            |t| t.settlement_authority = Address::new_from_array([9; 32]),
            |t| t.delivery_amount += 1,
            |t| t.payment_amount += 1,
            |t| t.expiry_timestamp += 1,
            |t| t.nonce += 1,
            |t| t.ref_string = [b'x'; 64],
            |t| t.seller_settlement_destination = destination(21),
            |t| t.buyer_settlement_destination = destination(21),
            |t| t.earliest_settlement_timestamp = None,
            |t| t.seller_refund_destination = destination(21),
            |t| t.buyer_refund_destination = destination(21),
            |t| t.creator = Address::new_from_array([9; 32]),
            |t| t.blinding = [0; 32],
        ];
        for (i, mutate) in mutations.iter().enumerate() {
            let mut terms = fixed_terms();
            mutate(&mut terms);
            assert_ne!(terms, base, "mutation {i} changed nothing");
            assert_ne!(terms.dvp_id().unwrap(), base_id, "mutation {i}");
        }
        let mut shifted = fixed_terms();
        shifted.earliest_settlement_timestamp = Some(1_700_000_001);
        assert_ne!(shifted.dvp_id().unwrap(), base_id);
    }

    #[test]
    fn negative_timestamps_are_rejected() {
        let mut terms = fixed_terms();
        terms.expiry_timestamp = -1;
        assert!(terms.dvp_id().is_err());
        let mut terms = fixed_terms();
        terms.earliest_settlement_timestamp = Some(-1);
        assert!(terms.dvp_id().is_err());
    }

    #[test]
    fn blinding_seeds_are_domain_separated() {
        let terms = fixed_terms();
        let create = terms.create_blinding_seed().unwrap();
        let settle = terms.settle_blinding_seed().unwrap();
        let abort = terms.abort_blinding_seed().unwrap();
        assert_ne!(create, settle);
        assert_ne!(create, abort);
        assert_ne!(settle, abort);
        assert_eq!(
            create,
            poseidon(&[&u64_right_align(u64::from(CREATE_TAG)), &terms.blinding]).unwrap()
        );
    }

    #[test]
    fn dvp_authority_matches_program() {
        let terms = fixed_terms();
        let (pda, bump) = terms.dvp_authority().unwrap();
        let derived = dvp_program::instructions::shared::dvp_authority(
            &terms.creator,
            &terms.dvp_id().unwrap(),
            bump,
        )
        .unwrap();
        assert_eq!(pda, derived);
    }

    #[test]
    fn validate_rejects_bad_terms() {
        assert!(fixed_terms().validate().is_ok());
        let p256 = ShieldedAddress {
            signing_pubkey: PublicKey::from_p256(
                &ViewingKey::from_bytes(&[3; 32]).unwrap().pubkey(),
            ),
            ..destination(11)
        };
        let invalid: [fn(&mut SwapDvp); 6] = [
            |t| t.delivery_amount = 0,
            |t| t.payment_amount = 0,
            |t| t.payment_mint = t.delivery_mint,
            |t| t.expiry_timestamp = -1,
            |t| t.earliest_settlement_timestamp = Some(t.expiry_timestamp + 1),
            |t| t.earliest_settlement_timestamp = Some(-1),
        ];
        for (i, mutate) in invalid.iter().enumerate() {
            let mut terms = fixed_terms();
            mutate(&mut terms);
            assert!(terms.validate().is_err(), "case {i}");
        }
        let terms = SwapDvp {
            buyer_refund_destination: p256,
            ..fixed_terms()
        };
        assert!(terms.validate().is_err());
    }
}
