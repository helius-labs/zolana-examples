use crate::shared::field_decimal;
use anyhow::{bail, ensure, Result};
use dvp_program::circuits::{
    DvpDestinationClient as DvpDestination, SwapDvpTerms as RawTerms,
    SwapDvpTermsClient as SwapDvpTerms,
};
use dvp_program::{ABORT_TAG, CREATE_TAG, DVP_AUTHORITY_PDA_SEED, SETTLEMENT_TAG};
use solana_address::Address;
use zolana_hasher::primitives::{hash_bytes, right_align, solana_owner_identity};
use zolana_keypair::{hash::poseidon, shielded::SHIELDED_ADDRESS_LEN, ShieldedAddress};
use zolana_program::{to_bytes, ClientInputs, InputRecords};
use zolana_transaction::Mint;

use crate::err;

/// Byte length of [`SwapDvp::to_bytes`].
pub const SWAP_DVP_LEN: usize = 5 * 32 + 4 * 8 + 64 + 4 * SHIELDED_ADDRESS_LEN + 1 + 8 + 2 * 32;

/// The DvP terms (spec "SwapDvpTerms"). Every party learns them from the encrypted
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
    /// proving work: the create checks and non-negative timestamps.
    ///
    /// It also requires the settlement authority to be a third party, as the
    /// reference DvP program does. The circuits do not check this; it keeps a
    /// party from settling or cancelling the trade alone.
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.settlement_authority != self.seller && self.settlement_authority != self.buyer,
            "the settlement authority is a party"
        );
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
        Ok(())
    }

    /// The DvP id, nested because Poseidon takes at most 12 inputs:
    ///
    /// ```text
    /// dvp_id = Poseidon(parties_hash, legs_hash, schedule_hash,
    ///                   destinations_hash, hash_bytes(ref_string), blinding)
    /// ```
    ///
    /// See [`swap_dvp_id`] for the inner hashes.
    pub fn dvp_id(&self) -> Result<[u8; 32]> {
        swap_dvp_id(SwapDvpTerms::try_from(self)?)
    }

    /// The per-DvP authority PDA `[b"dvp", creator, dvp_id]` and its bump.
    pub fn dvp_authority(&self) -> Result<(Address, u8)> {
        let dvp_id = self.dvp_id()?;
        Address::try_find_program_address(
            &[DVP_AUTHORITY_PDA_SEED, self.creator.as_ref(), &dvp_id],
            &dvp_program::ID,
        )
        .ok_or_else(|| anyhow::anyhow!("cannot derive the DvP authority"))
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
        poseidon(&[&right_align(&tag.to_be_bytes()), &self.blinding]).map_err(err)
    }
}

impl SwapDvp {
    pub fn circuit_terms(&self, delivery_mint: Mint, payment_mint: Mint) -> Result<SwapDvpTerms> {
        ensure!(
            delivery_mint.asset == self.delivery_mint,
            "delivery mint differs from the terms"
        );
        ensure!(
            payment_mint.asset == self.payment_mint,
            "payment mint differs from the terms"
        );
        Ok(SwapDvpTerms {
            seller_identity: field_decimal(
                solana_owner_identity(self.seller.as_array()).map_err(err)?,
            )?,
            buyer_identity: field_decimal(
                solana_owner_identity(self.buyer.as_array()).map_err(err)?,
            )?,
            settlement_authority_identity: field_decimal(
                solana_owner_identity(self.settlement_authority.as_array()).map_err(err)?,
            )?,
            creator_identity: field_decimal(
                solana_owner_identity(self.creator.as_array()).map_err(err)?,
            )?,
            delivery_mint,
            payment_mint,
            delivery_amount: self.delivery_amount,
            payment_amount: self.payment_amount,
            expiry_timestamp: timestamp(self.expiry_timestamp)?,
            has_earliest_settlement: self.earliest_settlement_timestamp.is_some(),
            earliest_settlement_timestamp: self
                .earliest_settlement_timestamp
                .map_or(Ok(0), timestamp)?,
            nonce: self.nonce,
            reference_hash: field_decimal(hash_bytes(&self.ref_string).map_err(err)?)?,
            seller_settlement_destination: destination_proof_input(
                &self.seller_settlement_destination,
            )?,
            buyer_settlement_destination: destination_proof_input(
                &self.buyer_settlement_destination,
            )?,
            seller_refund_destination: destination_proof_input(&self.seller_refund_destination)?,
            buyer_refund_destination: destination_proof_input(&self.buyer_refund_destination)?,
            blinding: field_decimal(self.blinding)?,
        })
    }
}

impl TryFrom<&SwapDvp> for SwapDvpTerms {
    type Error = anyhow::Error;
    fn try_from(terms: &SwapDvp) -> Result<Self> {
        let mint = |asset| {
            if asset == zolana_transaction::SOL_MINT {
                Mint::SOL
            } else {
                Mint::new(asset, 0)
            }
        };
        terms.circuit_terms(mint(terms.delivery_mint), mint(terms.payment_mint))
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

/// Compute the spec commitment from the circuit terms.
///
/// ```text
/// parties_hash      = Poseidon(id(seller), id(buyer), id(settlement_authority), id(creator))
/// legs_hash         = Poseidon(asset(delivery_mint), asset(payment_mint), delivery_amount, payment_amount)
/// schedule_hash     = Poseidon(expiry_timestamp, has_earliest, earliest_settlement_timestamp, nonce)
/// destinations_hash = Poseidon(dest(seller_settlement), dest(buyer_settlement), dest(seller_refund), dest(buyer_refund))
/// ```
fn swap_dvp_id(input: SwapDvpTerms) -> Result<[u8; 32]> {
    let terms = RawTerms::from_client(input, &mut InputRecords::new()).map_err(err)?;
    Ok(to_bytes(&terms.hash().map_err(err)?))
}

fn destination_proof_input(address: &ShieldedAddress) -> Result<DvpDestination> {
    Ok(DvpDestination {
        address: *address,
        viewing_key_hash: field_decimal(
            hash_bytes(address.viewing_pubkey.as_bytes()).map_err(err)?,
        )?,
    })
}

fn timestamp(value: i64) -> Result<u64> {
    u64::try_from(value).map_err(|_| anyhow::anyhow!("timestamp {value} is negative"))
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
    use zolana_keypair::{NullifierKey, PublicKey, ViewingKey};

    use super::*;

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
        assert_eq!(
            decimal(&terms.dvp_id().unwrap()),
            "13601006363444597634864084490330890110481277484464045593977180451704799918864"
        );
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
            poseidon(&[&right_align(&CREATE_TAG.to_be_bytes()), &terms.blinding]).unwrap()
        );
    }

    #[test]
    fn dvp_authority_matches_program() {
        let terms = fixed_terms();
        let (pda, bump) = terms.dvp_authority().unwrap();
        let derived = Address::create_program_address(
            &[
                DVP_AUTHORITY_PDA_SEED,
                terms.creator.as_ref(),
                &terms.dvp_id().unwrap(),
                &[bump],
            ],
            &dvp_program::ID,
        )
        .unwrap();
        assert_eq!(pda, derived);
    }

    /// An edit that makes valid terms invalid.
    type Mutation = fn(&mut SwapDvp);

    #[test]
    fn validate_rejects_bad_terms() {
        assert!(fixed_terms().validate().is_ok());
        let invalid: [(Mutation, &str); 8] = [
            (
                |t| t.settlement_authority = t.seller,
                "the settlement authority is a party",
            ),
            (
                |t| t.settlement_authority = t.buyer,
                "the settlement authority is a party",
            ),
            (|t| t.delivery_amount = 0, "delivery amount is zero"),
            (|t| t.payment_amount = 0, "payment amount is zero"),
            (
                |t| t.payment_mint = t.delivery_mint,
                "delivery and payment mints are equal",
            ),
            (|t| t.expiry_timestamp = -1, "expiry timestamp is negative"),
            (
                |t| t.earliest_settlement_timestamp = Some(t.expiry_timestamp + 1),
                "earliest settlement timestamp is after expiry",
            ),
            (
                |t| t.earliest_settlement_timestamp = Some(-1),
                "earliest settlement timestamp is negative",
            ),
        ];
        for (mutate, expected) in invalid {
            let mut terms = fixed_terms();
            mutate(&mut terms);
            let error = terms.validate().expect_err(expected);
            assert_eq!(error.to_string(), expected);
        }
        let destinations: [fn(&mut SwapDvp) -> &mut ShieldedAddress; 4] = [
            |t| &mut t.seller_settlement_destination,
            |t| &mut t.buyer_settlement_destination,
            |t| &mut t.seller_refund_destination,
            |t| &mut t.buyer_refund_destination,
        ];
        for destination in destinations {
            let mut terms = fixed_terms();
            let address = destination(&mut terms);
            *address = ShieldedAddress::for_pda(
                &Address::new_from_array([6; 32]),
                [0; 32],
                address.viewing_pubkey,
            );
            assert!(terms.validate().is_ok());
        }
    }

    fn decimal(bytes: &[u8; 32]) -> String {
        num_bigint::BigUint::from_bytes_be(bytes).to_string()
    }

    fn destination(seed: u8) -> ShieldedAddress {
        ShieldedAddress {
            signing_pubkey: PublicKey::from_ed25519(&[seed; 32]),
            nullifier_pubkey: NullifierKey::from_secret([seed; 31]).pubkey().unwrap(),
            viewing_pubkey: ViewingKey::from_bytes(&[seed; 32]).pubkey(),
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
}
