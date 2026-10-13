use private_staking_program::{pda, settlement::cached_utxo};
use solana_address::Address;
use zolana_keypair::{shielded::ShieldedAddress, NullifierKey, P256Pubkey, PublicKey};
use zolana_transaction::{
    utxo::{derive_output_blinding_seed, derive_transact_output_blinding},
    Mint, SppProofOutputUtxo,
};

#[test]
fn cached_utxo_matches_spp_output_commitment_nullifier_and_blinding() {
    let config = pda::config(&Address::new_from_array([10; 32])).0;
    let vault = pda::sol_vault(&config).0;
    let first_nullifier = [3; 32];
    let seed = derive_output_blinding_seed(&first_nullifier, &[9; 32]).unwrap();
    let tree_id = 7;
    let cached = cached_utxo(&vault, &first_nullifier, &seed, tree_id).unwrap();

    let zero = NullifierKey::from_secret([0; 31]);
    let mut output = SppProofOutputUtxo::new(
        Mint::SOL,
        0,
        ShieldedAddress {
            signing_pubkey: PublicKey::from_pda(&vault),
            nullifier_pubkey: zero.pubkey().unwrap(),
            viewing_pubkey: P256Pubkey::from_bytes(zolana_hasher::p256::P_CONST_SEC1).unwrap(),
        },
    )
    .unwrap();
    // TransferProver checks slot 0 against this derivation.
    output.blinding = derive_transact_output_blinding(&first_nullifier, &seed, 0).unwrap();
    let hash = output.hash(tree_id).unwrap();
    assert_eq!(
        (cached.blinding, cached.hash, cached.nullifier),
        (
            output.blinding,
            hash,
            zero.nullifier(&hash, &output.blinding).unwrap()
        )
    );
    assert_eq!(
        output.owner_tag,
        Some(vault.to_bytes()),
        "withdrawal output tag is Inline(sol_vault)"
    );
    assert_ne!(
        cached_utxo(&vault, &first_nullifier, &seed, tree_id + 1)
            .unwrap()
            .hash,
        cached.hash
    );
}
