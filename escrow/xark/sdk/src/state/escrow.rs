use anyhow::Result;
use spr_keypair::ShieldedAddress;
use spr_transaction::{Mint, SppProofOutputUtxo, WalletUtxo};
use timelock_escrow_program::circuits::EscrowTermsClient;

use crate::{err, escrow_authority, shared::field};

/// The escrow terms: who created the escrow (and receives the change and the
/// withdrawal) and when it unlocks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EscrowTerms {
    pub creator: ShieldedAddress,
    pub unlock_timestamp: u64,
}

impl EscrowTerms {
    /// The terms as the circuits take them: the creator's owner hash and the
    /// unlock timestamp.
    pub fn circuit_terms(&self) -> Result<EscrowTermsClient> {
        Ok(EscrowTermsClient {
            owner_hash: field(self.creator.owner_hash().map_err(err)?)?,
            unlock: self.unlock_timestamp,
        })
    }

    /// `Poseidon(owner_hash, unlock)`, the escrow UTXO's data hash, which the
    /// circuits recompute from the terms.
    pub fn data_hash(&self) -> Result<[u8; 32]> {
        let terms = self.circuit_terms()?;
        let terms = timelock_escrow_program::circuits::EscrowTerms {
            owner_hash: terms.owner_hash,
            unlock: terms.unlock,
        };
        Ok(spr_program::to_bytes(&terms.data_hash().map_err(err)?))
    }
}

/// The escrow UTXO: `amount` of `asset` owned by the escrow-authority PDA
/// under the zero nullifier key, with the terms' hash as its data hash and
/// no ciphertext data. Its opening (terms and blinding) is the complete
/// spend capability; the program signs for the PDA only through `withdraw`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EscrowUtxo {
    pub terms: EscrowTerms,
    /// Derived by the escrow transaction from its first nullifier, blinding
    /// seed and output slot.
    pub blinding: [u8; 32],
    pub asset: Mint,
    pub amount: u64,
}

impl EscrowUtxo {
    /// The escrow output. The owner's viewing key does not enter the
    /// commitment; the creator's is used so it can read the output.
    pub fn output_utxo(&self) -> Result<SppProofOutputUtxo> {
        Ok(SppProofOutputUtxo {
            asset: self.asset,
            amount: self.amount,
            blinding: self.blinding,
            owner_address: Some(
                escrow_authority()
                    .address(self.terms.creator.viewing_pubkey)
                    .map_err(err)?,
            ),
            data_hash: Some(self.terms.data_hash()?),
            ..Default::default()
        })
    }

    /// The escrow UTXO's commitment in tree `tree_id`.
    pub fn hash(&self, tree_id: u16) -> Result<[u8; 32]> {
        self.output_utxo()?.hash(tree_id).map_err(err)
    }

    /// The escrow UTXO as an SPP input at `leaf_index` of tree `tree_id`.
    /// Anyone holding the opening can build it; spending needs the PDA
    /// signature only the program gives.
    pub fn to_input_utxo(&self, tree_id: u16, leaf_index: u64) -> Result<WalletUtxo> {
        escrow_authority()
            .input(&self.output_utxo()?, tree_id, leaf_index)
            .map_err(err)
    }
}
