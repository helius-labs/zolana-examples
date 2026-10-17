use dvp_program::error::DvpError::*;

#[test]
fn error_codes_are_stable() {
    let table = [
        (ProofVerificationFailed as u32, 10000),
        (InvalidInstructionData as u32, 10001),
        (InvalidShieldedPoolProgram as u32, 10002),
        (MissingDvpAuthority as u32, 10003),
        (InvalidDvpAuthority as u32, 10004),
        (HashingFailed as u32, 10005),
        (TimestampOutOfWindow as u32, 10006),
    ];
    for (got, want) in table {
        assert_eq!(got, want, "error code drifted");
    }
}
