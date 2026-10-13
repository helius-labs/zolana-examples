use private_staking_program::verifying_keys;
use private_staking_prover::{CircuitId, PROVER};
use sha2::{Digest, Sha256};
use zolana_gnark_ffi_prover::Circuit;

/// The committed verifying keys pin the proving keys the tests generate, so a
/// circuit change without a regenerated key fails here instead of in a proof.
#[test]
fn committed_verifying_keys_match_the_generated_proving_keys() {
    for circuit in CircuitId::ALL {
        let dir = PROVER.keys_dir(*circuit);
        if !dir.join("pk.bin").exists() {
            PROVER.setup_insecure_test_keys(*circuit, &dir).unwrap();
        }
        let actual: [u8; 32] = Sha256::digest(std::fs::read(dir.join("pk.bin")).unwrap()).into();
        let expected = match circuit {
            CircuitId::Intent => verifying_keys::intent::VERIFYINGKEY_PROVING_KEY_SHA256,
            CircuitId::Cancel => verifying_keys::cancel::VERIFYINGKEY_PROVING_KEY_SHA256,
            CircuitId::Settlement => verifying_keys::settlement::VERIFYINGKEY_PROVING_KEY_SHA256,
        };
        assert_eq!(actual, expected, "{} proving key drifted", circuit.name());
    }
}
