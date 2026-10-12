use private_staking_program::{pda, settlement::payout_address};
use solana_address::Address;
use zolana_client::ProofInputUtxo;
use zolana_hasher::{
    primitives::{hash_bytes, right_align},
    Hasher, Poseidon,
};
use zolana_keypair::{NullifierKey, PublicKey};

#[test]
fn payout_address_matches_spp_address_input_commitment_and_nullifier() {
    let config = pda::config(&Address::new_from_array([10; 32])).0;
    let epoch = pda::epoch(&config, 42, 0).0;
    let batch = pda::batch(&epoch, 1).0;
    let vault = pda::sol_vault(&config).0;
    let zero = NullifierKey::from_secret([0; 31]);
    let owner =
        zolana_keypair::hash::owner_hash(&PublicKey::from_pda(&vault), &zero.pubkey().unwrap())
            .unwrap();
    let seed = hash_bytes(batch.as_array()).unwrap();
    let address = ProofInputUtxo {
        domain: right_align(&[2]),
        tree_id: right_align(&7u16.to_be_bytes()),
        owner_hash: owner,
        blinding: seed,
        ..Default::default()
    };
    let expected = zero.nullifier(&address.hash().unwrap(), &seed).unwrap();
    assert_eq!(payout_address(&config, &batch, 7).unwrap(), expected);
    assert_ne!(payout_address(&config, &batch, 8).unwrap(), expected);
    assert_ne!(
        payout_address(&config, &pda::batch(&epoch, 2).0, 7).unwrap(),
        expected
    );
    assert_ne!(expected, Poseidon::hashv(&[&[0; 32]]).unwrap());
}
