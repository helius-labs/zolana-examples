mod shared;

use anyhow::{ensure, Result};
use dvp_program::{error::DvpError, DVP_ADDRESS_TREE_ID};
use dvp_sdk::{
    index::{index_terms, DiscoveredDvp},
    state::DvpUtxo,
};
use shared::{assert_dvp_error, setup, INDEX_TIMEOUT};

#[test]
fn create_dvp_and_discover_terms() -> Result<()> {
    let env = setup(11)?;
    // D6: the create circuit pins the address slot to tree 0, so the
    // localnet tree the address is proven against must be tree 0.
    assert_eq!(env.localnet.tree_id, DVP_ADDRESS_TREE_ID);
    let terms = env.terms(1)?;
    let create = env.prove_create(&terms)?;
    let dvp_utxo_hash = create
        .spp
        .outputs
        .first()
        .map(|output| output.utxo_hash)
        .ok_or_else(|| anyhow::anyhow!("create transact has no output"))?;
    let create_ix = create.instruction()?;
    let create_signature = env.send(create_ix.clone(), &[&env.seller.solana], None)?;

    // Each party finds its message by its own pubkey, decrypts the terms and
    // recomputes the DvP UTXO the transaction committed.
    let expected_utxo = DvpUtxo::new(terms.clone(), env.localnet.tree_id);
    assert_eq!(expected_utxo.hash()?, dvp_utxo_hash);
    for (recipient_index, party) in (0u32..).zip(env.parties()) {
        let found = index_terms(&party.shielded, &env.localnet.client, INDEX_TIMEOUT)?;
        let [discovered] = found.as_slice() else {
            anyhow::bail!("party {recipient_index} discovered {} DvPs", found.len());
        };
        assert_eq!(
            discovered,
            &DiscoveredDvp {
                dvp_utxo: expected_utxo.clone(),
                leaf_index: discovered.leaf_index,
                recipient_index,
                tx_signature: create_signature,
            }
        );
    }

    // The address is indexed now, so no second proof of its non-inclusion
    // exists: resend the same signed instruction instead, made distinct by a
    // priority fee. Only failure is asserted, since the SPP error depends on
    // whether the nullifier is still queued.
    ensure!(
        env.send(create_ix, &[&env.seller.solana], Some(1)).is_err(),
        "duplicate create unexpectedly succeeded"
    );
    Ok(())
}

#[test]
fn create_dvp_rejects_a_mismatched_dvp_id() -> Result<()> {
    let env = setup(12)?;
    let terms = env.terms(1)?;
    let other_terms = env.terms(2)?;
    let (other_pda, other_bump) = other_terms.dvp_authority()?;
    let mut create = env.prove_create(&terms)?;
    // The other terms' id with its own bump derives a valid PDA, but the
    // create proof binds the original id and owner hash.
    create.dvp_id = other_terms.dvp_id()?;
    create.bump = other_bump;
    ensure!(
        other_pda != terms.dvp_authority()?.0,
        "the two DvPs share a PDA"
    );
    assert_dvp_error(
        "create with a mismatched dvp_id",
        env.send(create.instruction()?, &[&env.seller.solana], None),
        DvpError::ProofVerificationFailed,
    );
    Ok(())
}
