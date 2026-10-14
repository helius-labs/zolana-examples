use anyhow::{ensure, Result};
use zolana_keypair::{NullifierKey, PublicKey};
use zolana_transaction::{
    utxo::{SppProofInputUtxo, Utxo},
    Data, Mint,
};

use crate::{err, instructions::create_dvp::CreateDvpDerivation, state::SwapDvp};

/// The DvP UTXO the create transaction appends: a zero-amount SOL UTXO owned
/// by the DvP authority PDA with the zero nullifier secret, whose data hash
/// is `dvp_id` and whose blinding is derived from the create seed. It carries
/// no ciphertext (D9): every party recomputes it from the terms and the tree
/// it was appended to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DvpUtxo {
    pub terms: SwapDvp,
    /// Raw id of the tree holding the UTXO; the commitment folds it in.
    pub tree_id: u16,
}

impl DvpUtxo {
    pub fn new(terms: SwapDvp, tree_id: u16) -> Self {
        Self { terms, tree_id }
    }

    /// The create derivation, which holds the UTXO, its blinding and its hash.
    pub fn derivation(&self) -> Result<CreateDvpDerivation> {
        CreateDvpDerivation::new(&self.terms, self.tree_id)
    }

    pub fn hash(&self) -> Result<[u8; 32]> {
        Ok(self.derivation()?.dvp_utxo_hash)
    }

    /// `Poseidon(hash, blinding, 0)`: the UTXO's nullifier under the zero
    /// nullifier secret, and the first nullifier of the settle and abort
    /// transactions that spend it.
    pub fn nullifier(&self) -> Result<[u8; 32]> {
        let derivation = self.derivation()?;
        zero_nullifier_key()
            .nullifier(&derivation.dvp_utxo_hash, &derivation.dvp_utxo.blinding)
            .map_err(err)
    }

    /// The UTXO as an SPP input at `leaf_index`. Anyone holding the terms can
    /// build it; spending needs the PDA signature only the program gives.
    pub fn to_input_utxo(&self, leaf_index: u64) -> Result<SppProofInputUtxo> {
        let derivation = self.derivation()?;
        let utxo = Utxo {
            owner: PublicKey::from_pda(&derivation.dvp_authority),
            asset: Mint::SOL,
            amount: 0,
            blinding: derivation.dvp_utxo.blinding,
            ring_program_id: None,
            data: Data::default(),
        };
        let key = zero_nullifier_key();
        let nullifier_pubkey = key.pubkey().map_err(err)?;
        let utxo_hash = utxo
            .hash(
                &nullifier_pubkey,
                &derivation.dvp_id,
                &[0; 32],
                self.tree_id,
            )
            .map_err(err)?;
        ensure!(
            utxo_hash == derivation.dvp_utxo_hash,
            "DvP input utxo does not hash to the created DvP UTXO"
        );
        let nullifier = key.nullifier(&utxo_hash, &utxo.blinding).map_err(err)?;
        Ok(SppProofInputUtxo {
            utxo,
            utxo_hash,
            nullifier,
            nullifier_pubkey,
            data_hash: Some(derivation.dvp_id),
            ring_data_hash: None,
            tree_id: self.tree_id,
            leaf_index,
            cache_slot: None,
            compact: false,
        })
    }
}

/// The public zero-secret nullifier key of every PDA-owned DvP UTXO.
fn zero_nullifier_key() -> NullifierKey {
    NullifierKey::from_secret([0u8; 31])
}

#[cfg(test)]
mod tests {
    use zolana_program::compression::ZERO_NULLIFIER_PUBKEY;

    use super::*;
    use crate::index::fixture::terms_fixture;

    #[test]
    fn input_utxo_spends_the_created_dvp_utxo() {
        let dvp_utxo = terms_fixture().dvp_utxo;
        let derivation = dvp_utxo.derivation().expect("derivation");
        let input = dvp_utxo.to_input_utxo(9).expect("input utxo");
        assert_eq!(
            (
                input.utxo_hash,
                input.nullifier,
                input.nullifier_pubkey,
                input.leaf_index
            ),
            (
                derivation.dvp_utxo_hash,
                dvp_utxo.nullifier().expect("nullifier"),
                ZERO_NULLIFIER_PUBKEY,
                9
            )
        );
    }

    #[test]
    fn the_tree_id_changes_the_commitment() {
        let dvp_utxo = terms_fixture().dvp_utxo;
        let moved = DvpUtxo::new(dvp_utxo.terms.clone(), dvp_utxo.tree_id + 1);
        assert_ne!(
            dvp_utxo.hash().expect("hash"),
            moved.hash().expect("moved hash")
        );
    }
}
