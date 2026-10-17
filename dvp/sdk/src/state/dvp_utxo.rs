use anyhow::{ensure, Result};
use zolana_program::ProgramOwner;
use zolana_transaction::WalletUtxo;

use crate::{err, instructions::create_dvp::CreateDvpDerivation, state::SwapDvp};

/// The DvP UTXO the create transaction appends: a zero-amount SOL UTXO owned
/// by the DvP authority PDA with the zero nullifier secret, whose data hash
/// is `dvp_id` and whose blinding is derived from the create seed. It carries
/// no ciphertext: every party recomputes it from the terms and the tree
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
        ProgramOwner::nullifier_key()
            .nullifier(&derivation.dvp_utxo_hash, &derivation.dvp_utxo.blinding)
            .map_err(err)
    }

    /// The UTXO as an SPP input at `leaf_index`. Anyone holding the terms can
    /// build it; spending needs the PDA signature only the program gives.
    pub fn to_input_utxo(&self, leaf_index: u64) -> Result<WalletUtxo> {
        let derivation = self.derivation()?;
        let input = ProgramOwner::new(derivation.dvp_authority)
            .input(&derivation.dvp_utxo, self.tree_id, leaf_index)
            .map_err(err)?;
        ensure!(
            input.utxo_hash == derivation.dvp_utxo_hash,
            "DvP input utxo does not hash to the created DvP UTXO"
        );
        Ok(input)
    }
}

#[cfg(test)]
mod tests {
    use zolana_program::ProgramOwner;

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
                ProgramOwner::nullifier_pubkey().unwrap(),
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
