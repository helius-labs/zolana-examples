#[test]
fn circuits_encode_only_their_specified_utxo_inputs() {
    // 1. Inspect the fixed input schema passed to each compiled circuit.
    let cases = [
        (
            "create",
            shared::create_inputs(&shared::sample_terms())
                .proof_inputs()
                .encoded(),
            0,
        ),
        (
            "reclaim",
            shared::reclaim_inputs(false).proof_inputs().encoded(),
            1,
        ),
        (
            "settle",
            shared::settle_inputs([0, 0]).proof_inputs().encoded(),
            3,
        ),
        (
            "abort",
            shared::abort_inputs(false, false, true, true)
                .proof_inputs()
                .encoded(),
            3,
        ),
        (
            "unfunded abort",
            shared::abort_inputs(false, false, false, false)
                .proof_inputs()
                .encoded(),
            3,
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

mod abort_dvp_circuit;
mod create_dvp_circuit;
mod reclaim_dvp_circuit;
mod settle_dvp_circuit;
mod shared;
