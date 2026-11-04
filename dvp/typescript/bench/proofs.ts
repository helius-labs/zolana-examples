import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { readFileSync, writeFileSync } from "node:fs";
import { availableParallelism, cpus, platform, arch } from "node:os";
import { performance } from "node:perf_hooks";
import { fileURLToPath } from "node:url";
import { after } from "node:test";
import { gzipSync } from "node:zlib";

import type { PoolOptions, WorkerPool } from "../../target/zk/dvp-program/wasm/xark/pool.d.ts";
import { circuitArtifacts, ProgramProver, useWorkerPool } from "../src/wasm.js";

const { createPool }: { createPool(options: PoolOptions): Promise<WorkerPool> } = await import(
  new URL("../../target/zk/dvp-program/wasm/xark/pool.mjs", import.meta.url).href
);

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
  // 0. Prove with a worker pool: DVP_WASM_THREADS workers, all cores by default,
  //    or the single-threaded session for 0.
  const requested = process.env.DVP_WASM_THREADS;
  const threads = requested === undefined ? availableParallelism() : Number(requested);
  assert.ok(Number.isInteger(threads) && threads >= 0, "DVP_WASM_THREADS is a worker count");
  const pool = threads
    ? await createPool({
        threads,
        wasm: fileURLToPath(
          new URL("../../target/zk/dvp-program/wasm/dvp_program.js", import.meta.url),
        ),
      })
    : undefined;
  useWorkerPool(pool);
  if (pool) after(() => pool.terminate());

  // 1. Record the machine and exact compiled WASM artifact.
  const artifact = new URL("../../target/zk/dvp-program/wasm/dvp_program_bg.wasm", import.meta.url);
  const artifactBytes = readFileSync(artifact);
  const artifactHash = sha256(artifactBytes);
  const metadata = {
    date: new Date().toISOString(),
    commit: execFileSync("git", ["rev-parse", "HEAD"], {
      cwd: fileURLToPath(new URL("../..", import.meta.url)),
      encoding: "utf8",
    }).trim(),
    node: process.version,
    machine: `${platform()} ${arch()}; ${cpus().at(0)?.model ?? "unknown CPU"}; ${cpus().length} logical CPUs`,
    wasmSha256: artifactHash,
    wasmBytes: artifactBytes.byteLength,
    wasmGzipBytes: gzipSync(artifactBytes, { level: 9 }).byteLength,
    samples: SAMPLES,
    threads,
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
    if (pool)
      assert.ok(pool.stats.fftCalls > 0 && pool.stats.msmCalls > 0, "workers were not used");
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
    wasmBytes: number;
    wasmGzipBytes: number;
    threads: number;
  },
  timings: Record<CircuitName, Timings>,
): string {
  const proving = metadata.threads ? `${metadata.threads} workers` : "Xark session";
  const rows = CIRCUITS.map((name) => {
    const measured = timings[name];
    return `| ${name} | ${median(measured.keyReadAndChecksumMs).toFixed(1)} | ${measured.sessionLoadMs?.toFixed(1)} | ${measured.firstProofMs?.toFixed(1)} | ${median(measured.warmProofMs).toFixed(1)} | ${Math.min(...measured.warmProofMs).toFixed(1)}–${Math.max(...measured.warmProofMs).toFixed(1)} |`;
  });
  return `# TypeScript WASM — ${proving}\n\nMeasured ${metadata.date} at commit \`${metadata.commit}\`.\n\n${metadata.machine}; Node.js ${metadata.node}. The generated Node.js WASM build uses the release profile and ${metadata.threads ? `Xark's streaming prover with a pool of ${metadata.threads} workers` : "a synchronous Xark proving session"}.\n\n| Circuit | Artifact read + SHA-256 (ms) | Prover load (ms) | First prove (ms) | Warm prove median (ms) | Warm prove range (ms) |\n| --- | ---: | ---: | ---: | ---: | ---: |\n${rows.join("\n")}\n\nEach warm figure uses five samples after one additional warm-up, with the same loaded prover and proof inputs. The fixtures cover create, a full delivery reclaim, a settlement with surplus, and cancellation with both deposits. Abort's first call uses an unfunded cancellation; its warm samples use both deposits.\n\nProver load includes checked artifact reads, verifying key decoding, and Xark circuit and key preparation. \`prove\` includes proof generation and the prover's internal verification. Artifact read figures include the TypeScript loader's checksum check and use the warmed filesystem cache. Transaction construction, SDK encryption, WASM module loading, worker startup, SPP proofs, and network requests are excluded. The existing circuit tests and public-input assertions must pass before this report is written.\n\nCombined WASM binary: ${(metadata.wasmBytes / 1048576).toFixed(2)} MiB (${metadata.wasmBytes.toLocaleString("en-US")} bytes); gzip -9: ${(metadata.wasmGzipBytes / 1048576).toFixed(2)} MiB (${metadata.wasmGzipBytes.toLocaleString("en-US")} bytes).\n\nWorker-pool measurements are in [THREAD_BENCHMARK.md](THREAD_BENCHMARK.md).\n\nMachine timings can vary with background load. Raw samples are in \`../target/wasm-proof-benchmark.json\`. WASM SHA-256: \`${metadata.wasmSha256}\`.\n\nReproduce from \`dvp/typescript\`:\n\n\`\`\`sh\nnpx --no-install tsx bench/proofs.ts\n\`\`\`\n\nSet \`DVP_WASM_THREADS\` to choose the worker count, or \`0\` for the single-threaded session.\n`;
}

await main();
