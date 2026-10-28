import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { readFileSync, writeFileSync } from "node:fs";
import { cpus, platform, arch } from "node:os";
import { performance } from "node:perf_hooks";
import { fileURLToPath } from "node:url";

import { circuitArtifacts, ProgramProver } from "../src/wasm.js";

const CIRCUITS = ["create_dvp", "reclaim_dvp", "settle_dvp", "abort_dvp"] as const;
const SAMPLES = 5;
type CircuitName = (typeof CIRCUITS)[number];

interface Timings {
  keyReadAndChecksumMs: number[];
  sessionLoadMs?: number;
  firstProofMs?: number;
  warmProofMs: number[];
  calls: number;
}

async function main(): Promise<void> {
  // 1. Record the machine and exact compiled WASM artifact.
  const artifact = new URL("../../target/zk/dvp-program/wasm/dvp_program_bg.wasm", import.meta.url);
  const artifactHash = sha256(readFileSync(artifact));
  const metadata = {
    date: new Date().toISOString(),
    commit: execFileSync("git", ["rev-parse", "HEAD"], {
      cwd: fileURLToPath(new URL("../..", import.meta.url)),
      encoding: "utf8",
    }).trim(),
    node: process.version,
    machine: `${platform()} ${arch()}; ${cpus().at(0)?.model ?? "unknown CPU"}; ${cpus().length} logical CPUs`,
    wasmSha256: artifactHash,
    samples: SAMPLES,
    threads: 1,
  };
  const timings: Record<CircuitName, Timings> = {
    create_dvp: newTimings(),
    reclaim_dvp: newTimings(),
    settle_dvp: newTimings(),
    abort_dvp: newTimings(),
  };

  // 2. Measure checked key reads, then instrument this process's WASM provers.
  for (const name of CIRCUITS) {
    for (let sample = 0; sample < SAMPLES; sample++) {
      const start = performance.now();
      circuitArtifacts(name);
      timings[name].keyReadAndChecksumMs.push(performance.now() - start);
    }
  }
  for (const name of CIRCUITS) {
    const started = performance.now();
    const prover = new ProgramProver(name);
    timings[name].sessionLoadMs = performance.now() - started;
    prover.free();
  }
  instrument(timings);

  // 3. Reuse the existing circuit fixtures and their correctness assertions.
  process.once("exit", (code) => {
    if (code !== 0) return;
    for (const name of CIRCUITS) {
      assert.equal(timings[name].warmProofMs.length, SAMPLES, `missing ${name} samples`);
      assert.ok(timings[name].sessionLoadMs !== undefined);
      assert.ok(timings[name].firstProofMs !== undefined);
    }
    assert.equal(sha256(readFileSync(artifact)), artifactHash, "WASM changed during the run");
    writeFileSync(
      new URL("../../target/wasm-proof-benchmark.json", import.meta.url),
      JSON.stringify({ metadata, timings }, null, 2) + "\n",
    );
    writeFileSync(new URL("../BENCHMARK.md", import.meta.url), report(metadata, timings));
    execFileSync(
      fileURLToPath(new URL("../node_modules/.bin/oxfmt", import.meta.url)),
      ["--write", fileURLToPath(new URL("../BENCHMARK.md", import.meta.url))],
      { stdio: "ignore" },
    );
    console.log("Recorded TypeScript WASM timings in typescript/BENCHMARK.md");
  });
  await import("../test/circuits.test.js");
}

function newTimings(): Timings {
  return { keyReadAndChecksumMs: [], warmProofMs: [], calls: 0 };
}

function instrument(timings: Record<CircuitName, Timings>): void {
  const prove = ProgramProver.prototype.prove;
  ProgramProver.prototype.prove = function (transaction) {
    const timingsForCircuit = timings[this.name];
    const start = performance.now();
    const proof = prove.call(this, transaction);
    timingsForCircuit.firstProofMs ??= performance.now() - start;
    timingsForCircuit.calls++;

    // Abort fixture call 4 is cancellation with both deposits funded.
    const representative = timingsForCircuit.calls === (this.name === "abort_dvp" ? 4 : 1);
    if (representative) {
      assert.deepEqual(prove.call(this, transaction).publicInputs, proof.publicInputs);
      for (let sample = 0; sample < SAMPLES; sample++) {
        const start = performance.now();
        const result = prove.call(this, transaction);
        timingsForCircuit.warmProofMs.push(performance.now() - start);
        assert.deepEqual(result.publicInputs, proof.publicInputs);
      }
      console.log(
        `${this.name}: warm proof median ${median(timingsForCircuit.warmProofMs).toFixed(1)} ms`,
      );
    }
    return proof;
  };
}

function median(values: readonly number[]): number {
  const value = [...values].sort((left, right) => left - right).at(Math.floor(values.length / 2));
  assert.ok(value !== undefined);
  return value;
}

function sha256(bytes: Uint8Array): string {
  return createHash("sha256").update(bytes).digest("hex");
}

function report(
  metadata: {
    date: string;
    commit: string;
    node: string;
    machine: string;
    wasmSha256: string;
    threads: number;
  },
  timings: Record<CircuitName, Timings>,
): string {
  const rows = CIRCUITS.map((name) => {
    const measured = timings[name];
    return `| ${name} | ${median(measured.keyReadAndChecksumMs).toFixed(1)} | ${measured.sessionLoadMs?.toFixed(1)} | ${measured.firstProofMs?.toFixed(1)} | ${median(measured.warmProofMs).toFixed(1)} | ${Math.min(...measured.warmProofMs).toFixed(1)}–${Math.max(...measured.warmProofMs).toFixed(1)} |`;
  });
  return `# TypeScript WASM — Xark session\n\nMeasured ${metadata.date} at commit \`${metadata.commit}\`.\n\n${metadata.machine}; Node.js ${metadata.node}. The generated Node.js WASM build uses the release profile and a synchronous Xark proving session.\n\n| Circuit | Artifact read + SHA-256 (ms) | Prover load (ms) | First prove (ms) | Warm prove median (ms) | Warm prove range (ms) |\n| --- | ---: | ---: | ---: | ---: | ---: |\n${rows.join("\n")}\n\nEach warm figure uses five samples after one additional warm-up, with the same loaded prover and proof inputs. The fixtures cover create, a full delivery reclaim, a settlement with surplus, and cancellation with both deposits. Abort's first call uses an unfunded cancellation; its warm samples use both deposits.\n\nProver load includes checked artifact reads, proving and verifying key decoding, and Xark circuit preparation. \`prove\` includes proof generation and the prover's internal verification. Artifact read figures include the TypeScript loader's checksum check and use the warmed filesystem cache. Transaction construction, SDK encryption, WASM module loading, SPP proofs, and network requests are excluded. The existing circuit tests and public-input assertions must pass before this report is written.\n\nMachine timings can vary with background load. Raw samples are in \`../target/wasm-proof-benchmark.json\`. WASM SHA-256: \`${metadata.wasmSha256}\`.\n\nReproduce from \`dvp/typescript\`:\n\n\`\`\`sh\nnpx --no-install tsx bench/proofs.ts\n\`\`\`\n`;
}

await main();
