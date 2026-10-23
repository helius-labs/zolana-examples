use anyhow::{ensure, Result};
use solana_address::Address;
use zolana_keypair::{ShieldedAddress, ViewingKey, X25519Pubkey};
use zolana_program::ProgramOwner;
use zolana_transaction::{Mint, SppProofOutputUtxo, WalletUtxo};

use crate::state::SwapDvp;

/// The two legs of a DvP.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Leg {
    /// The seller's asset, `delivery_mint` / `delivery_amount`.
    Delivery,
    /// The buyer's asset, `payment_mint` / `payment_amount`.
    Payment,
}

impl Leg {
    pub fn mint(self, terms: &SwapDvp) -> Address {
        match self {
            Self::Delivery => terms.delivery_mint,
            Self::Payment => terms.payment_mint,
        }
    }

    /// The amount settlement moves on this leg; a deposit may hold more.
    pub fn amount(self, terms: &SwapDvp) -> u64 {
        match self {
            Self::Delivery => terms.delivery_amount,
            Self::Payment => terms.payment_amount,
        }
    }

    /// The leg whose mint is `mint`, if any. The mints differ
    /// ([`SwapDvp::validate`]), so at most one matches.
    pub fn of_mint(terms: &SwapDvp, mint: &Address) -> Option<Self> {
        [Self::Delivery, Self::Payment]
            .into_iter()
            .find(|leg| &leg.mint(terms) == mint)
    }
}

/// A funded leg: a plain UTXO a funder transferred to the DvP's deposit
/// address ([`Deposit::address`]). It is owned by the DvP authority PDA with
/// the zero nullifier secret, carries no data, and its ciphertext is
/// readable by the settlement authority (and by the funder through its
/// transaction viewing key).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Deposit {
    pub terms: SwapDvp,
    pub leg: Leg,
    /// The deposited asset: its mint address and pool asset id.
    pub mint: Mint,
    /// The deposited amount; anything above `leg.amount(terms)` is surplus
    /// that settlement refunds.
    pub amount: u64,
    pub blinding: [u8; 32],
    /// Raw id of the tree holding the UTXO; the commitment folds it in.
    pub tree_id: u16,
    pub leaf_index: u64,
}

impl Deposit {
    /// Where funders send a leg: the DvP authority PDA as owner, the zero
    /// nullifier pubkey (so the program can spend without a secret), and the
    /// settlement authority's viewing pubkey, which the deposit ciphertext is
    /// encrypted to. The viewing pubkey does not enter the owner hash, so
    /// every funder of the DvP sends to the same owner.
    pub fn address(
        terms: &SwapDvp,
        authority_viewing_pubkey: X25519Pubkey,
    ) -> Result<ShieldedAddress> {
        let (pda, _) = terms.dvp_authority()?;
        ProgramOwner::new(pda)
            .address(authority_viewing_pubkey)
            .map_err(crate::err)
    }

    /// The deposit as an SPP input. Anyone holding the UTXO fields can build it;
    /// spending needs the PDA signature only the program gives.
    pub fn to_input_utxo(&self) -> Result<WalletUtxo> {
        let (pda, _) = self.terms.dvp_authority()?;
        let owner = ProgramOwner::new(pda);
        let output = SppProofOutputUtxo {
            asset: self.mint,
            amount: self.amount,
            blinding: self.blinding,
            owner_address: Some(
                owner
                    .address(ViewingKey::new().pubkey())
                    .map_err(crate::err)?,
            ),
            ..Default::default()
        };
        owner
            .input(&output, self.tree_id, self.leaf_index)
            .map_err(crate::err)
    }

    pub fn hash(&self) -> Result<[u8; 32]> {
        Ok(self.to_input_utxo()?.utxo_hash)
    }

    /// Checks the deposit funds `leg` of `terms`: it belongs to those terms,
    /// names that leg, and holds the leg's mint. Its amount is not checked.
    pub(crate) fn check_leg(&self, terms: &SwapDvp, leg: Leg) -> Result<()> {
        ensure!(&self.terms == terms, "{leg:?} deposit of other terms");
        ensure!(self.leg == leg, "{leg:?} deposit funds the other leg");
        ensure!(
            self.mint.asset == leg.mint(terms),
            "{leg:?} deposit mint is not the leg's mint"
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use zolana_transaction::{instructions::transact::SppProofOutputUtxo, SOL_MINT};

    use super::*;
    use crate::index::fixture::{fixture_parties, sample_terms};

    #[test]
    fn legs_follow_the_terms() {
        let terms = sample_terms(&fixture_parties());
        assert_eq!(
            (
                Leg::of_mint(&terms, &terms.delivery_mint),
                Leg::of_mint(&terms, &SOL_MINT),
                Leg::of_mint(&terms, &Address::new_from_array([9; 32])),
                Leg::Delivery.amount(&terms),
                Leg::Payment.amount(&terms),
            ),
            (
                Some(Leg::Delivery),
                Some(Leg::Payment),
                None,
                terms.delivery_amount,
                terms.payment_amount,
            )
        );
    }

    #[test]
    fn input_utxo_spends_what_a_funder_sends_to_the_deposit_address() {
        let parties = fixture_parties();
        let terms = sample_terms(&parties);
        let [_, _, authority] = &parties;
        let address = Deposit::address(&terms, authority.viewing_pubkey()).unwrap();
        let blinding = zolana_keypair::random_blinding();
        let deposit = Deposit {
            terms: terms.clone(),
            leg: Leg::Payment,
            mint: Mint::SOL,
            amount: 3_000,
            blinding,
            tree_id: 1,
            leaf_index: 4,
        };
        let sent = SppProofOutputUtxo {
            asset: Mint::SOL,
            amount: 3_000,
            blinding,
            owner_address: Some(address),
            ..Default::default()
        };
        let input = deposit.to_input_utxo().unwrap();
        let (pda, _) = terms.dvp_authority().unwrap();
        assert_eq!(
            (
                input.utxo_hash,
                input.nullifier_pubkey,
                address.owner_hash().unwrap(),
                input.leaf_index,
            ),
            (
                sent.hash(1).unwrap(),
                ProgramOwner::nullifier_pubkey().unwrap(),
                *zolana_program::program::PdaOwner::new(&pda)
                    .unwrap()
                    .owner_hash(),
                4,
            )
        );
    }
}
