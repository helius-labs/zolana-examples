#!/usr/bin/env python3
"""Compare an example's gnark and Xark benchmarks side by side.

Usage: python3 test-utils/compare-benchmarks.py <example-dir>

Reads <example>/gnark/BENCHMARK.md and <example>/xark/BENCHMARK.md, both
written by their `bench_cu` tests in the shared benchmark format, and writes
<example>/BENCHMARK.md: one table per metric group with gnark, Xark and the
delta (Xark minus gnark). Fails if either file lacks a table or column, or
the two files measure different circuits or instructions. Standard library
only.
"""

import re
import sys
from pathlib import Path

IMPLEMENTATIONS = (("gnark", "gnark"), ("xark", "Xark"))
SETUP_FIELDS = ("Implementation", "SDK", "Shielded-pool program", "Machine")

# (table, row label, metric groups). Every column of a table except the first
# belongs to exactly one group.
TABLES = (
    (
        "Circuits",
        "Circuit",
        (
            (
                "Circuit size",
                (
                    "Constraints",
                    "Public inputs",
                    "Proving key (bytes)",
                    "Verifying key (bytes)",
                    "Proof (bytes)",
                ),
            ),
            (
                "Circuit proving",
                (
                    "Key load (ms)",
                    "Per-proof setup (ms)",
                    "Warm proving time (ms)",
                    "In-memory proving time (ms)",
                ),
            ),
        ),
    ),
    (
        "Summary",
        "Instruction",
        (
            ("Instruction circuit", ("Constraints",)),
            ("Compute units", ("Total CU", "Verify CU", "SPP CPI CU")),
            (
                "Instruction proving",
                (
                    "Warm circuit proof (ms)",
                    "Warm SPP proof (ms)",
                    "Warm total proving (ms)",
                ),
            ),
            (
                "Transaction size",
                ("Instruction data (bytes)", "Accounts", "v1 tx (bytes)"),
            ),
        ),
    ),
)


class BenchmarkError(Exception):
    pass


def cells(line):
    return [cell.strip() for cell in line.strip().strip("|").split("|")]


def parse(path):
    """The example and implementation named in the title, the setup bullets and
    the tables of one BENCHMARK.md."""
    if not path.is_file():
        raise BenchmarkError(f"{path} is missing; run its bench_cu test")
    lines = path.read_text().splitlines()
    title = lines[0] if lines else ""
    match = re.fullmatch(r"# (.+) \((gnark|Xark)\) -- Benchmark", title)
    if not match:
        raise BenchmarkError(f"{path}: unexpected title {title!r}")
    setup = {}
    tables = {}
    section = None
    for index, line in enumerate(lines):
        if line.startswith("## "):
            section = line[3:].strip()
        elif section is None and line.startswith("- ") and ": " in line:
            key, value = line[2:].split(": ", 1)
            setup[key] = value
        elif (
            section is not None
            and line.startswith("|")
            and section not in tables
            and index + 1 < len(lines)
            and re.fullmatch(r"\|[\s\-|]+\|", lines[index + 1].strip())
        ):
            header = cells(line)
            rows = []
            for row_line in lines[index + 2 :]:
                if not row_line.startswith("|"):
                    break
                row = cells(row_line)
                if len(row) != len(header):
                    raise BenchmarkError(f"{path}: row width differs in {section}: {row_line}")
                rows.append(row)
            tables[section] = (header, rows)
    return match.group(1), match.group(2), setup, tables


def number(path, table, value):
    try:
        return int(value.replace(",", ""))
    except ValueError:
        raise BenchmarkError(f"{path}: {table} value {value!r} is not an integer") from None


def thousands(value):
    return f"{value:,}"


def delta(gnark, xark):
    difference = xark - gnark
    sign = "+" if difference > 0 else ""
    if gnark == 0:
        return f"{sign}{difference:,}"
    return f"{sign}{difference:,} ({sign}{difference / gnark * 100:.1f}%)"


def table_lines(headers, rows):
    out = ["| " + " | ".join(headers) + " |"]
    out.append("| " + " | ".join("-" * len(header) for header in headers) + " |")
    out.extend("| " + " | ".join(row) + " |" for row in rows)
    return out


def check_table(name, label, groups, inputs):
    """Rows of `name` as {label value: {column: int}} per implementation, in
    the gnark file's row order."""
    expected = [label] + [column for _, columns in groups for column in columns]
    parsed = []
    for path, tables in inputs:
        if name not in tables:
            raise BenchmarkError(f"{path}: missing the {name} table")
        header, rows = tables[name]
        if header != expected:
            raise BenchmarkError(
                f"{path}: {name} columns {header} differ from the shared format {expected}"
            )
        values = {}
        for row in rows:
            key = row[0]
            if key in values:
                raise BenchmarkError(f"{path}: {name} lists {key} twice")
            values[key] = {
                column: number(path, name, value) for column, value in zip(header[1:], row[1:])
            }
        if not values:
            raise BenchmarkError(f"{path}: the {name} table is empty")
        parsed.append((path, [row[0] for row in rows], values))
    (gnark_path, order, gnark_values), (xark_path, _, xark_values) = parsed
    if set(gnark_values) != set(xark_values):
        raise BenchmarkError(
            f"{name}: {gnark_path} lists {sorted(gnark_values)}, "
            f"{xark_path} lists {sorted(xark_values)}"
        )
    return order, gnark_values, xark_values


def render(example_dir, example, setups, inputs):
    command = f"python3 test-utils/compare-benchmarks.py {example_dir.as_posix()}"
    out = [
        f"# {example} (gnark vs Xark) -- Benchmark",
        "",
        f"Generated by `{command}`; do not edit.",
        "",
        "Side-by-side view of [`gnark/BENCHMARK.md`](gnark/BENCHMARK.md) and "
        "[`xark/BENCHMARK.md`](xark/BENCHMARK.md), which their `bench_cu` tests "
        "generate in the shared format; see either file for the methodology. Delta is "
        "Xark minus gnark, with the percentage of the gnark value. The shielded-pool "
        "program differs between the implementations, so `SPP CPI CU` is not an "
        "apples-to-apples comparison; `Verify CU` and the circuit metrics are. Warm "
        "proving time is one full call of each implementation's prove function, which "
        "for Xark 0.3.0 includes per-proof setup (re-reading the circuit and rebuilding "
        "its constraint rows) and decoding the proving key from disk; in-memory proving "
        "time holds both in memory and is the prover-to-prover comparison. Timings from "
        "runs on different machines or under different load do not compare.",
        "",
        "## Setup",
        "",
    ]
    setup_rows = []
    for field in SETUP_FIELDS:
        row = [field]
        for (path, _), setup in zip(inputs, setups):
            if field not in setup:
                raise BenchmarkError(f"{path}: missing the `- {field}:` line")
            row.append(setup[field])
        setup_rows.append(row)
    out += table_lines(["Field", "gnark", "Xark"], setup_rows)
    for name, label, groups in TABLES:
        order, gnark_values, xark_values = check_table(name, label, groups, inputs)
        for group, columns in groups:
            rows = []
            for key in order:
                for column in columns:
                    gnark = gnark_values[key][column]
                    xark = xark_values[key][column]
                    rows.append(
                        [key, column, thousands(gnark), thousands(xark), delta(gnark, xark)]
                    )
            out += ["", f"## {group}", ""]
            out += table_lines([label, "Metric", "gnark", "Xark", "Delta"], rows)
    return "\n".join(out) + "\n"


def main(argv):
    if len(argv) != 2:
        print(__doc__.strip(), file=sys.stderr)
        return 2
    repo = Path(__file__).resolve().parent.parent
    example_dir = Path(argv[1])
    root = example_dir if example_dir.is_absolute() else repo / example_dir
    try:
        example_dir = root.resolve().relative_to(repo)
    except ValueError:
        pass
    try:
        examples, setups, inputs = [], [], []
        for directory, name in IMPLEMENTATIONS:
            path = root / directory / "BENCHMARK.md"
            example, implementation, setup, tables = parse(path)
            if implementation != name:
                raise BenchmarkError(f"{path}: title names {implementation}, expected {name}")
            examples.append(example)
            setups.append(setup)
            inputs.append((path, tables))
        if examples[0] != examples[1]:
            raise BenchmarkError(f"the files benchmark different examples: {examples}")
        output = root / "BENCHMARK.md"
        output.write_text(render(example_dir, examples[0], setups, inputs))
    except BenchmarkError as error:
        print(f"error: {error}", file=sys.stderr)
        return 1
    print(f"wrote {output}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
