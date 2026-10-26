#[test]
fn circuits_allocate_only_their_specified_utxo_inputs() {
    use zolana_program::{circuit::LabelKind, testing::constraint_labels};

    // 1. Inspect the compiled witness allocations for every circuit.
    let cases = [
        (
            "create",
            constraint_labels(&shared::create_inputs(&shared::sample_terms())).unwrap(),
            0,
        ),
        (
            "reclaim",
            constraint_labels(&shared::reclaim_inputs(false)).unwrap(),
            1,
        ),
        (
            "settle",
            constraint_labels(&shared::settle_inputs([0, 0])).unwrap(),
            3,
        ),
        (
            "abort",
            constraint_labels(&shared::abort_inputs(false, false, true, true)).unwrap(),
            3,
        ),
        (
            "unfunded abort",
            constraint_labels(&shared::abort_inputs(false, false, false, false)).unwrap(),
            3,
        ),
    ];

    // 2. Reject unused UTXO inputs introduced by a padding default.
    for (name, labels, expected) in cases {
        let inputs = labels
            .iter()
            .filter(|label| label.kind == LabelKind::Scope && label.text == "a utxo proof input")
            .count();
        assert_eq!(inputs, expected, "{name} allocates unexpected input UTXOs");
    }
}

mod abort_dvp_circuit;
mod create_dvp_circuit;
mod program_boundary;
mod reclaim_dvp_circuit;
mod settle_dvp_circuit;
mod shared;
