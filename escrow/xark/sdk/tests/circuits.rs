//! Circuit tests: each proves with the checksum-pinned artifacts in
//! `target/zk/timelock-escrow-program`, built by `zolana zk compile --test`.
mod escrow_circuit;
mod shared;
mod withdraw_circuit;

#[test]
fn circuits_encode_only_their_specified_utxo_inputs() {
    // 1. Inspect the fixed input schema passed to each compiled circuit.
    let cases = [
        (
            "escrow",
            shared::escrow_inputs(250, 750)
                .create_program_transaction()
                .unwrap()
                .proof_inputs
                .encoded(),
            1,
        ),
        (
            "withdraw",
            shared::withdraw_inputs()
                .create_program_transaction()
                .unwrap()
                .proof_inputs
                .encoded(),
            1,
        ),
    ];

    // 2. Reject unused UTXO inputs introduced by a padding default.
    for (name, leaves, expected) in cases {
        let inputs = leaves
            .keys()
            .filter(|name| name.ends_with(".domain"))
            .count();
        assert_eq!(inputs, expected, "{name} encodes unexpected input UTXOs");
    }
}
