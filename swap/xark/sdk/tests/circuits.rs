//! Circuit tests: each proves with the checksum-pinned artifacts in
//! `target/zk/swap-program`, built by `zolana zk compile --test`, and checks
//! the proof with the committed verifiers.
mod cancel_circuit;
mod make_circuit;
mod shared;
mod take_circuit;

#[test]
fn circuits_encode_only_their_specified_utxo_inputs() {
    // 1. Inspect the fixed input schema passed to each compiled circuit.
    let order = shared::order_utxo(shared::terms(&shared::keypair(), None));
    let cases = [
        (
            "make",
            shared::make_inputs(shared::SOURCE_AMOUNT, 750)
                .create_program_transaction()
                .unwrap()
                .proof_inputs
                .encoded(),
            1,
        ),
        (
            "take",
            shared::take_inputs(&order, shared::keypair())
                .create_program_transaction()
                .unwrap()
                .proof_inputs
                .encoded(),
            2,
        ),
        (
            "cancel",
            shared::cancel_inputs()
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
