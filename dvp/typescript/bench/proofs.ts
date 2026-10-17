import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { readFileSync, writeFileSync } from "node:fs";
import { cpus, platform, arch } from "node:os";
import { performance } from "node:perf_hooks";
import { fileURLToPath } from "node:url";

import { provingKey, initWasm, type Proof } from "../src/wasm.js";

const CIRCUITS = ["create_dvp", "reclaim_dvp", "settle_dvp", "abort_dvp"] as const;
const SAMPLES = 5;
type CircuitName = (typeof CIRCUITS)[number];

interface Timings {
  keyReadAndChecksumMs: number[];
  fromKeyMs?: number;
  firstProofMs?: number;
  warmProofMs: number[];
  calls: number;
}

interface Prover {
  prove(inputs: Uint8Array): Proof;
}

interface ProverClass<P extends Prover> {
  fromKey(key: Uint8Array, checksum: Uint8Array): P;
  prototype: P;
}

async function main(): Promise<void> {
  const threads = process.env.DVP_WASM_THREADS;
  const wasm = await initWasm(
    threads === undefined || threads === "all" ? {} : { threads: Number(threads) },
  );
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
    threads: wasm.proverThreads,
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
      provingKey(name);
      timings[name].keyReadAndChecksumMs.push(performance.now() - start);
    }
  }
  instrument("create_dvp", wasm.CreateDvpProver, timings.create_dvp);
  instrument("reclaim_dvp", wasm.ReclaimDvpProver, timings.reclaim_dvp);
  instrument("settle_dvp", wasm.SettleDvpProver, timings.settle_dvp);
  instrument("abort_dvp", wasm.AbortDvpProver, timings.abort_dvp);

  // 3. Reuse the existing circuit fixtures and their correctness assertions.
  process.once("exit", (code) => {
    if (code !== 0) return;
    for (const name of CIRCUITS) {
      assert.equal(timings[name].warmProofMs.length, SAMPLES, `missing ${name} samples`);
      assert.ok(timings[name].fromKeyMs !== undefined);
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

function instrument<P extends Prover>(
  name: CircuitName,
  constructor: ProverClass<P>,
  timings: Timings,
): void {
  const fromKey = constructor.fromKey;
  constructor.fromKey = function (key, checksum) {
    const start = performance.now();
    const prover = fromKey.call(constructor, key, checksum);
    timings.fromKeyMs ??= performance.now() - start;
    return prover;
  };
  const prove = constructor.prototype.prove;
  constructor.prototype.prove = function (inputs) {
    const start = performance.now();
    const proof = prove.call(this, inputs);
    timings.firstProofMs ??= performance.now() - start;
    timings.calls++;

    // Abort fixture call 4 is cancellation with both deposits funded.
    const representative = timings.calls === (name === "abort_dvp" ? 4 : 1);
    if (representative) {
      assert.deepEqual(prove.call(this, inputs).publicInputs, proof.publicInputs);
      for (let sample = 0; sample < SAMPLES; sample++) {
        const start = performance.now();
        const result = prove.call(this, inputs);
        timings.warmProofMs.push(performance.now() - start);
        assert.deepEqual(result.publicInputs, proof.publicInputs);
      }
      console.log(`${name}: warm proof median ${median(timings.warmProofMs).toFixed(1)} ms`);
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
    return `| ${name} | ${median(measured.keyReadAndChecksumMs).toFixed(1)} | ${measured.fromKeyMs?.toFixed(1)} | ${measured.firstProofMs?.toFixed(1)} | ${median(measured.warmProofMs).toFixed(1)} | ${Math.min(...measured.warmProofMs).toFixed(1)}–${Math.max(...measured.warmProofMs).toFixed(1)} |`;
  });
  return `# TypeScript WASM — ${metadata.threads} threads\n\nMeasured ${metadata.date} at commit \`${metadata.commit}\`.\n\n${metadata.machine}; Node.js ${metadata.node}. The generated Node.js WASM build uses the release profile and ${metadata.threads} prover threads.\n\n| Circuit | Key read + SHA-256 (ms) | First fromKey (ms) | First prove (ms) | Warm prove median (ms) | Warm prove range (ms) |\n| --- | ---: | ---: | ---: | ---: | ---: |\n${rows.join("\n")}\n\nEach warm figure uses five samples after one additional warm-up, with the same loaded prover and proof inputs. The fixtures cover create, a full delivery reclaim, a settlement with surplus, and cancellation with both deposits. Abort's first call uses an unfunded cancellation; its warm samples use both deposits.\n\n\`fromKey\` includes WASM checksum validation, key decoding, and initial circuit preparation. \`prove\` includes proof generation and the prover's internal verification. Key read figures include the TypeScript loader's checksum check and use the warmed filesystem cache. Transaction construction, SDK encryption, WASM module and worker-pool loading, SPP proofs, and network requests are excluded. The existing circuit tests and public-input assertions must pass before this report is written.\n\nMachine timings can vary with background load. Raw samples are in \`../target/wasm-proof-benchmark.json\`. WASM SHA-256: \`${metadata.wasmSha256}\`.\n\nReproduce from \`dvp/typescript\`:\n\n\`\`\`sh\nnpx --no-install tsx bench/proofs.ts\n\`\`\`\n`;
}

await main();
