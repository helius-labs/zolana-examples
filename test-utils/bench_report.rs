//! The benchmark every gnark and Xark example writes to its `BENCHMARK.md`:
//! the measurement helpers and the renderer, shared by the four
//! `test/tests/bench_cu.rs` benches through
//! `#[path = "../../../../test-utils/bench_report.rs"] mod bench_report;`, so
//! the files of one example compare line by line and
//! `test-utils/compare-benchmarks.py` can read them.
//!
//! A bench that includes this module defines the child-process half of the
//! key-load measurement as an ignored test named `key_load_probe` that calls
//! [`time_key_load`].
use std::{
    fmt::Write as _,
    process::Command,
    time::{Duration, Instant},
};

use light_program_profiler::report::ProfileEntry;

/// Timed proofs per median, after one untimed warm-up proof.
const PROOF_SAMPLES: usize = 5;
/// Cold key loads per median, each in a fresh child process.
const KEY_LOAD_SAMPLES: usize = 5;

/// Runs the bench, from the implementation directory. The gnark crates have
/// no committed lockfile, so the command does not pass `--locked`.
const BENCH_COMMAND: &str =
    "cargo test --release --manifest-path test/Cargo.toml --test bench_cu -- --ignored --nocapture";

const GNARK_IMPLEMENTATION: &str = "gnark (Go circuits, spr-gnark-ffi-prover)";
const GNARK_SDK: &str = "Zolana Rust SDK 0.8.0";
const GNARK_SHIELDED_POOL_PROGRAM: &str = "Zolana Rust SDK 0.8.0";
const GNARK_PROVER_NOTE: &str = "The gnark prover keeps the compiled constraint system and the \
parsed proving key in memory for the life of the process.";

const XARK_IMPLEMENTATION: &str = "Xark (Rust circuits, spr program SDK)";
const XARK_SDK: &str = "Zolana 98948daa1, Xark 0.3.0";
const XARK_SHIELDED_POOL_PROGRAM: &str = "Zolana 98948daa1";
const XARK_PROVER_NOTE: &str =
    "The Xark prover streams the proving key from its file during each proof.";

/// The test of the bench binary that runs one cold key load.
const KEY_LOAD_PROBE_TEST: &str = "key_load_probe";
/// Names the circuit the `key_load_probe` child process loads.
const KEY_LOAD_CIRCUIT_ENV: &str = "BENCH_KEY_LOAD_CIRCUIT";
/// Prefixes the line on which the child process reports its load time.
const KEY_LOAD_RESULT_PREFIX: &str = "BENCH_KEY_LOAD_NS=";

// Each bench constructs only its own implementation.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Implementation {
    Gnark,
    Xark,
}

impl Implementation {
    fn name(self) -> &'static str {
        match self {
            Self::Gnark => "gnark",
            Self::Xark => "Xark",
        }
    }

    fn detail(self) -> &'static str {
        match self {
            Self::Gnark => GNARK_IMPLEMENTATION,
            Self::Xark => XARK_IMPLEMENTATION,
        }
    }

    fn sdk(self) -> &'static str {
        match self {
            Self::Gnark => GNARK_SDK,
            Self::Xark => XARK_SDK,
        }
    }

    fn shielded_pool_program(self) -> &'static str {
        match self {
            Self::Gnark => GNARK_SHIELDED_POOL_PROGRAM,
            Self::Xark => XARK_SHIELDED_POOL_PROGRAM,
        }
    }

    /// How the in-process prover holds its proving key between proofs.
    fn prover_note(self) -> &'static str {
        match self {
            Self::Gnark => GNARK_PROVER_NOTE,
            Self::Xark => XARK_PROVER_NOTE,
        }
    }
}

/// One circuit's row of the Circuits table.
pub struct CircuitRow {
    pub name: String,
    pub constraints: u64,
    pub public_inputs: u64,
    pub proving_key_bytes: u64,
    pub verifying_key_bytes: u64,
    /// [`proof_bytes`] of the proof the instruction submits.
    pub proof_bytes: u64,
    /// [`cold_key_load`].
    pub key_load: Duration,
    /// [`warm_median`] of the in-process circuit proof.
    pub warm_proof: Duration,
}

/// One instruction's measurements: its Summary row and its CU section.
pub struct InstructionRow {
    pub name: String,
    /// The circuit the instruction's program proof is proved with.
    pub circuit: String,
    pub entries: Vec<ProfileEntry>,
    /// [`warm_median`] of the SPP proof on the running prover server.
    pub warm_spp_proof: Duration,
    pub instruction_data_bytes: u64,
    pub accounts: u64,
    pub v1_tx_bytes: u64,
}

pub struct Report {
    /// `Timelock Escrow`, `Confidential Swap`.
    pub example: &'static str,
    pub implementation: Implementation,
    pub circuits: Vec<CircuitRow>,
    pub instructions: Vec<InstructionRow>,
}

/// Runs `run` once untimed, then `PROOF_SAMPLES` times timed, and returns the
/// last result with the median time.
pub fn warm_median<T>(mut run: impl FnMut() -> T) -> (T, Duration) {
    let mut last = run();
    let mut samples = Vec::with_capacity(PROOF_SAMPLES);
    for _ in 0..PROOF_SAMPLES {
        let start = Instant::now();
        last = run();
        samples.push(start.elapsed());
    }
    (last, median(samples))
}

/// The byte length of a Groth16 proof as the instruction submits it: its
/// serialized a, b and c points.
pub fn proof_bytes(a: &[u8], b: &[u8], c: &[u8]) -> u64 {
    u64::try_from(a.len() + b.len() + c.len()).expect("proof length fits u64")
}

/// Loads `circuit`'s proving artifacts cold `KEY_LOAD_SAMPLES` times, each in
/// a fresh child process of this test binary running its `key_load_probe`
/// test, so no load reuses what an earlier one left in memory. Returns the
/// median load time and each child process's standard output.
pub fn cold_key_load(circuit: &str) -> (Duration, Vec<String>) {
    let (samples, outputs) = (0..KEY_LOAD_SAMPLES)
        .map(|_| key_load_child(circuit))
        .unzip();
    (median(samples), outputs)
}

fn key_load_child(circuit: &str) -> (Duration, String) {
    let output = Command::new(std::env::current_exe().expect("test binary path"))
        .args([
            KEY_LOAD_PROBE_TEST,
            "--exact",
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(KEY_LOAD_CIRCUIT_ENV, circuit)
        .output()
        .expect("run key load probe");
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    assert!(
        output.status.success(),
        "key load probe for {circuit} failed:\n{stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    // The test harness prints the test name on the same line, before it.
    let nanos = stdout
        .split_once(KEY_LOAD_RESULT_PREFIX)
        .and_then(|(_, rest)| {
            rest.chars()
                .take_while(char::is_ascii_digit)
                .collect::<String>()
                .parse::<u64>()
                .ok()
        })
        .unwrap_or_else(|| panic!("key load probe for {circuit} printed no time:\n{stdout}"));
    (Duration::from_nanos(nanos), stdout)
}

/// The child-process half of [`cold_key_load`]: times `load` on the circuit
/// the parent names and reports the time. The result of `load` is dropped
/// after timing. Does nothing when the test runs outside [`cold_key_load`].
pub fn time_key_load<T>(load: impl FnOnce(&str) -> T) {
    let Ok(circuit) = std::env::var(KEY_LOAD_CIRCUIT_ENV) else {
        return;
    };
    let start = Instant::now();
    let loaded = load(&circuit);
    let elapsed = start.elapsed();
    drop(loaded);
    println!("{KEY_LOAD_RESULT_PREFIX}{}", elapsed.as_nanos());
}

fn median(mut samples: Vec<Duration>) -> Duration {
    samples.sort_unstable();
    samples
        .get(samples.len() / 2)
        .copied()
        .expect("at least one sample")
}

/// `1234567` as `1,234,567`.
fn thousands(value: u64) -> String {
    let digits = value.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index != 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

fn millis(duration: Duration) -> String {
    thousands(u64::try_from(duration.as_millis()).expect("milliseconds fit u64"))
}

/// CPU model, logical core count and OS of the machine running the bench.
fn machine() -> String {
    let cpu = command_output("sysctl", &["-n", "machdep.cpu.brand_string"])
        .or_else(|| {
            std::fs::read_to_string("/proc/cpuinfo")
                .ok()
                .and_then(|info| {
                    info.lines()
                        .find(|line| line.starts_with("model name"))
                        .and_then(|line| line.split_once(':'))
                        .map(|(_, model)| model.trim().to_string())
                })
        })
        .unwrap_or_else(|| "unknown CPU".into());
    let cores = std::thread::available_parallelism().map_or(0, usize::from);
    let os = command_output("uname", &["-sr"]).unwrap_or_else(|| std::env::consts::OS.into());
    format!("{cpu}, {cores} cores, {os} ({})", std::env::consts::ARCH)
}

fn command_output(program: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(program).args(args).output().ok()?;
    let text = String::from_utf8(output.stdout).ok()?;
    let text = text.trim();
    (output.status.success() && !text.is_empty()).then(|| text.to_string())
}

fn table(out: &mut String, headers: &[&str], rows: &[Vec<String>]) {
    let line = |cells: Vec<String>| format!("| {} |\n", cells.join(" | "));
    out.push_str(&line(headers.iter().map(|h| h.to_string()).collect()));
    out.push_str(&line(headers.iter().map(|h| "-".repeat(h.len())).collect()));
    for row in rows {
        assert_eq!(row.len(), headers.len(), "table row width");
        out.push_str(&line(row.clone()));
    }
}

/// The instruction's entry for the profiled function `matches` selects,
/// which it must call exactly once.
fn entry(instruction: &InstructionRow, matches: impl Fn(&str) -> bool) -> &ProfileEntry {
    let mut found = instruction
        .entries
        .iter()
        .filter(|entry| matches(&entry.func_name));
    let entry = found
        .next()
        .unwrap_or_else(|| panic!("{}: profiled function missing", instruction.name));
    assert!(
        found.next().is_none(),
        "{}: profiled function {} recorded twice",
        instruction.name,
        entry.func_name
    );
    entry
}

fn is_spp_cpi(name: &str) -> bool {
    name.starts_with("cpi_spp_transact")
}

fn title_case(name: &str) -> String {
    let mut chars = name.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

impl Report {
    fn render(&self) -> String {
        let implementation = self.implementation;
        let mut out = format!(
            "# {} ({}) -- Benchmark\n\nGenerated by `{BENCH_COMMAND}`; do not edit.\n\n",
            self.example,
            implementation.name()
        );
        let _ = write!(
            out,
            "Compute units come from replaying each instruction under mollusk with the profiling \
             build of the example program (`profile-program`, light-program-profiler). Only the \
             example program is profiled: `process_<ix>_ix` is the whole instruction, \
             `verify_<ix>_proof` the program proof verification (public-input hashing and Groth16 \
             verification), and the shielded-pool program is a plain build whose CPI cost is the \
             `cpi_spp_transact*` row. The shielded-pool program differs between the gnark and Xark \
             implementations, so `SPP CPI CU` is not an apples-to-apples comparison; `Verify CU` \
             and the circuit columns are. Proving times are warm release-build medians of \
             {PROOF_SAMPLES} proofs after 1 warm-up proof: the program circuit proof in process \
             with its artifacts loaded before timing, the SPP proof against the running prover \
             server with its keys loaded. Key load is the median of {KEY_LOAD_SAMPLES} cold loads \
             of a circuit's proving artifacts into the in-process prover until it can prove (gnark: \
             compile the constraint system and read the proving key; Xark: read the \
             checksum-checked circuit bytecode and proving key), each in a fresh child process of \
             the bench; the SPP prover's key load is not measured. {} Proof bytes are the \
             serialized Groth16 proof points the instruction submits. Transaction sizes are those \
             of the serialized v1 transaction carrying the instruction.\n\n",
            implementation.prover_note()
        );
        let _ = write!(
            out,
            "- Implementation: {}\n- SDK: {}\n- Shielded-pool program: {}\n- Machine: {}\n\n",
            implementation.detail(),
            implementation.sdk(),
            implementation.shielded_pool_program(),
            machine()
        );
        let _ = write!(out, "Regenerate with `{BENCH_COMMAND}`.\n\n");

        out.push_str("## Circuits\n\n");
        let circuit_rows: Vec<Vec<String>> = self
            .circuits
            .iter()
            .map(|circuit| {
                vec![
                    circuit.name.clone(),
                    thousands(circuit.constraints),
                    thousands(circuit.public_inputs),
                    thousands(circuit.proving_key_bytes),
                    thousands(circuit.verifying_key_bytes),
                    thousands(circuit.proof_bytes),
                    millis(circuit.key_load),
                    millis(circuit.warm_proof),
                ]
            })
            .collect();
        table(
            &mut out,
            &[
                "Circuit",
                "Constraints",
                "Public inputs",
                "Proving key (bytes)",
                "Verifying key (bytes)",
                "Proof (bytes)",
                "Key load (ms)",
                "Warm proving time (ms)",
            ],
            &circuit_rows,
        );

        out.push_str("\n## Summary\n\n");
        let summary_rows: Vec<Vec<String>> = self
            .instructions
            .iter()
            .map(|instruction| {
                let circuit = self
                    .circuits
                    .iter()
                    .find(|circuit| circuit.name == instruction.circuit)
                    .unwrap_or_else(|| panic!("no circuit {}", instruction.circuit));
                let process = format!("process_{}_ix", instruction.name);
                let verify = format!("verify_{}_proof", instruction.name);
                vec![
                    instruction.name.clone(),
                    thousands(circuit.constraints),
                    thousands(entry(instruction, |name| name == process).total_cu),
                    thousands(entry(instruction, |name| name == verify).total_cu),
                    thousands(entry(instruction, is_spp_cpi).total_cu),
                    millis(circuit.warm_proof),
                    millis(instruction.warm_spp_proof),
                    millis(circuit.warm_proof + instruction.warm_spp_proof),
                    thousands(instruction.instruction_data_bytes),
                    thousands(instruction.accounts),
                    thousands(instruction.v1_tx_bytes),
                ]
            })
            .collect();
        table(
            &mut out,
            &[
                "Instruction",
                "Constraints",
                "Total CU",
                "Verify CU",
                "SPP CPI CU",
                "Warm circuit proof (ms)",
                "Warm SPP proof (ms)",
                "Warm total proving (ms)",
                "Instruction data (bytes)",
                "Accounts",
                "v1 tx (bytes)",
            ],
            &summary_rows,
        );

        for instruction in &self.instructions {
            let process = format!("process_{}_ix", instruction.name);
            let verify = format!("verify_{}_proof", instruction.name);
            let mut entries: Vec<&ProfileEntry> = instruction.entries.iter().collect();
            // The instruction, its SPP CPI, its proof verification, then any
            // other profiled function by name.
            entries.sort_by_key(|entry| {
                let rank = if entry.func_name == process {
                    0
                } else if is_spp_cpi(&entry.func_name) {
                    1
                } else if entry.func_name == verify {
                    2
                } else {
                    3
                };
                (rank, entry.func_name.clone())
            });
            let rows: Vec<Vec<String>> = entries
                .iter()
                .map(|entry| {
                    vec![
                        format!("`{}`", entry.func_name),
                        thousands(entry.total_cu),
                        thousands(entry.net_cu),
                    ]
                })
                .collect();
            let _ = write!(out, "\n## {}\n\n", title_case(&instruction.name));
            table(&mut out, &["Function", "Total CU", "Net CU"], &rows);
        }
        out
    }

    /// Writes the report to the implementation's `BENCHMARK.md`.
    pub fn write(&self, path: &str) {
        std::fs::write(path, self.render()).unwrap_or_else(|e| panic!("write {path}: {e}"));
    }
}
