//! The size of a gnark circuit as compiled by the prover it proves with,
//! for the gnark benches, included from their `test/tests/bench_cu.rs` with
//! `#[path = "../../../../test-utils/gnark_circuit_size.rs"] mod gnark_circuit_size;`.
//!
//! gnark logs the counts while the prover's preload compiles the constraint
//! system: `nbPublic` (public inputs) when it parses the circuit and
//! `nbConstraints` (the compiled R1CS `GetNbConstraints`) when it builds it.
//! The key-load child processes capture that log.

/// The compiled circuit's R1CS sizes.
pub struct CircuitSize {
    pub constraints: u64,
    pub public_inputs: u64,
}

/// The circuit size every key-load output logs, checked against the
/// public-input count of the circuit's raw verifying key `vk`.
pub fn circuit_size(name: &str, outputs: &[String], vk: &[u8]) -> CircuitSize {
    let logs: Vec<String> = outputs.iter().map(|output| strip_ansi(output)).collect();
    let size = |field: &str| {
        let mut values = logs.iter().map(|log| log_field(log, field));
        let value = values
            .next()
            .unwrap_or_else(|| panic!("{name}: no key-load output"));
        assert!(
            values.all(|other| other == value),
            "{name}: key loads disagree on `{field}`"
        );
        value
    };
    let size = CircuitSize {
        constraints: size("nbConstraints"),
        public_inputs: size("nbPublic"),
    };
    assert_eq!(
        verifying_key_public_inputs(vk),
        size.public_inputs,
        "{name}: verifying key and circuit disagree on public inputs"
    );
    size
}

/// Removes ANSI color escapes from gnark's console log.
fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            for next in chars.by_ref() {
                if next.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// The value of `field`, which the log must give exactly one value.
fn log_field(log: &str, field: &str) -> u64 {
    let key = format!("{field}=");
    let mut values = log.split(key.as_str()).skip(1).map(|rest| {
        rest.chars()
            .take_while(char::is_ascii_digit)
            .collect::<String>()
            .parse::<u64>()
            .unwrap_or_else(|_| panic!("unparsable `{field}` in gnark log"))
    });
    let value = values
        .next()
        .unwrap_or_else(|| panic!("gnark log has no `{field}`:\n{log}"));
    assert!(
        values.all(|other| other == value),
        "gnark logged more than one `{field}`"
    );
    value
}

/// Public inputs committed by a gnark raw verifying key: `len(K) - 1`. The
/// raw encoding is alpha_1, beta_1, beta_2, gamma_2, delta_1, delta_2 (G1 64
/// bytes, G2 128 bytes), then `len(K)` as a big-endian u32.
fn verifying_key_public_inputs(vk: &[u8]) -> u64 {
    const K_LEN_OFFSET: usize = 3 * 64 + 3 * 128;
    let len: [u8; 4] = vk
        .get(K_LEN_OFFSET..K_LEN_OFFSET + 4)
        .and_then(|bytes| bytes.try_into().ok())
        .expect("verifying key too short");
    u64::from(u32::from_be_bytes(len))
        .checked_sub(1)
        .expect("verifying key has no K points")
}
