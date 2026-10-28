import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { availableParallelism, cpus } from "node:os";
import { performance } from "node:perf_hooks";
import { fileURLToPath } from "node:url";
import type { PoolOptions, WorkerPool } from "../../target/zk/dvp-program/wasm/xark/pool.d.ts";
import { circuitArtifacts, ProgramProver, wasm, type ProgramTransaction } from "../src/wasm.js";

const { createPool }: { createPool(options: PoolOptions): Promise<WorkerPool> } = await import(
  new URL("../../target/zk/dvp-program/wasm/xark/pool.mjs", import.meta.url).href
);

const NAMES = ["create_dvp", "reclaim_dvp", "settle_dvp", "abort_dvp"] as const;
type Name = (typeof NAMES)[number];
type Fixture = Pick<ProgramTransaction, "proofInputs" | "publicHash">;
interface Measurement {
  name: Name;
  workers: number;
  loadMs: number;
  samplesMs: number[];
  medianMs: number;
  fftCalls: number;
  msmCalls: number;
}
const artifacts = new URL("../../target/zk/dvp-program/", import.meta.url);
const fixturesPath = new URL("../../target/wasm-thread-fixtures.json", import.meta.url);
const samples = 3;

async function capture(): Promise<void> {
  const fixtures: Partial<Record<Name, Fixture>> = {};
  const calls: Partial<Record<Name, number>> = {};
  const original = ProgramProver.prototype.prove;
  ProgramProver.prototype.prove = function (transaction) {
    calls[this.name] = (calls[this.name] ?? 0) + 1;
    // Abort call four is cancellation with both deposits funded.
    if (this.name === "abort_dvp" ? calls[this.name] === 4 : !fixtures[this.name]) {
      fixtures[this.name] = {
        proofInputs: transaction.proofInputs,
        publicHash: transaction.publicHash,
      };
      writeFileSync(
        fixturesPath,
        JSON.stringify(fixtures, (_key, value) =>
          value instanceof Map
            ? Object.fromEntries(value)
            : value instanceof Uint8Array
              ? Array.from(value)
              : typeof value === "bigint"
                ? value.toString()
                : value,
        ),
      );
    }
    return original.call(this, transaction);
  };
  await import("../test/circuits.test.js");
}

async function measure(workers: number): Promise<void> {
  const fixtures: Record<Name, Fixture> = JSON.parse(readFileSync(fixturesPath, "utf8"));
  const pool = workers
    ? await createPool({
        threads: workers,
        wasm: fileURLToPath(new URL("wasm/dvp_program.js", artifacts)),
      })
    : undefined;
  const measurements: Measurement[] = [];
  try {
    for (const name of NAMES) {
      const checked = circuitArtifacts(name);
      const verifier = new wasm.VerifyingKey(checked.verifyingKey);
      const fixture = fixtures[name];
      assert.ok(fixture, `missing ${name} fixture`);
      fixture.publicHash = Uint8Array.from(fixture.publicHash);
      let session: InstanceType<typeof wasm.ProvingSession> | undefined;
      const started = performance.now();
      if (pool) {
        const rows = new URL(`${name}/r1cs.stream`, artifacts);
        wasm.preloadStreaming(checked.circuit, checked.provingKey, {
          sources: checked.sources,
          ...(existsSync(rows) ? { r1cs: readFileSync(rows) } : {}),
        });
      } else {
        session = new wasm.ProvingSession(checked.circuit, checked.provingKey, checked.sources);
      }
      const loadMs = performance.now() - started;
      const before = { fft: pool?.stats.fftCalls ?? 0, msm: pool?.stats.msmCalls ?? 0 };
      const proofs = new Set<string>();
      const samplesMs: number[] = [];
      try {
        for (let sample = 0; sample <= samples; sample++) {
          const start = performance.now();
          assert.ok(pool || session);
          const bundle = pool
            ? wasm.proveStreamingPreloaded(fixture.proofInputs, { pool })
            : session!.prove(fixture.proofInputs);
          try {
            assert.ok(verifier.verify(bundle.proof, bundle.publicInputs));
            const converted = wasm.programProof(
              bundle.proof,
              bundle.publicInputs,
              fixture.publicHash,
            );
            assert.deepEqual(converted.publicInputs, [fixture.publicHash]);
            const elapsed = performance.now() - start;
            const proof = Buffer.from(bundle.proof).toString("hex");
            assert.ok(!proofs.has(proof), "proof randomness repeated");
            proofs.add(proof);
            if (sample) samplesMs.push(elapsed);
            else {
              const wrong = fixture.publicHash.slice();
              wrong[0] ^= 1;
              assert.throws(() => wasm.programProof(bundle.proof, bundle.publicInputs, wrong));
            }
          } finally {
            bundle.free();
          }
        }
        const fftCalls = (pool?.stats.fftCalls ?? 0) - before.fft;
        const msmCalls = (pool?.stats.msmCalls ?? 0) - before.msm;
        if (workers) assert.ok(fftCalls > 0 && msmCalls > 0, "worker kernels were not used");
        const medianMs = [...samplesMs].sort((left, right) => left - right).at(1);
        assert.ok(medianMs !== undefined);
        const measured = { name, workers, loadMs, samplesMs, medianMs, fftCalls, msmCalls };
        measurements.push(measured);
        console.log(JSON.stringify(measured));
      } finally {
        session?.free();
        verifier.free();
      }
    }
  } finally {
    await pool?.terminate();
  }
  writeFileSync(
    new URL(`../../target/wasm-thread-${workers}.json`, import.meta.url),
    JSON.stringify(measurements, null, 2),
  );
}

async function main(): Promise<void> {
  const counts = [
    ...new Set([
      0,
      Math.min(4, availableParallelism()),
      Math.min(8, availableParallelism()),
      availableParallelism(),
    ]),
  ];
  const runner = fileURLToPath(new URL("../node_modules/.bin/tsx", import.meta.url));
  const script = fileURLToPath(import.meta.url);
  const artifact = new URL("wasm/dvp_program_bg.wasm", artifacts);
  const sha256 = () => createHash("sha256").update(readFileSync(artifact)).digest("hex");
  const wasmSha256 = sha256();
  const metadata = {
    date: new Date().toISOString(),
    commit: execFileSync("git", ["rev-parse", "HEAD"], { encoding: "utf8" }).trim(),
    node: process.version,
    machine: cpus().at(0)?.model,
    samples,
    wasmSha256,
  };
  execFileSync(runner, [script, "capture"], { stdio: "inherit" });
  const configurations = counts.map((workers) => {
    execFileSync(runner, [script, "measure", String(workers)], { stdio: "inherit" });
    return JSON.parse(
      readFileSync(new URL(`../../target/wasm-thread-${workers}.json`, import.meta.url), "utf8"),
    ) as Measurement[];
  });
  assert.equal(sha256(), wasmSha256, "WASM changed during the run");
  writeFileSync(
    new URL("../../target/wasm-thread-benchmark.json", import.meta.url),
    JSON.stringify({ metadata, configurations }, null, 2),
  );
  const rows = NAMES.map(
    (name) =>
      `| ${name} | ${configurations.map((configuration) => configuration.find((entry) => entry.name === name)!.medianMs.toFixed(1)).join(" | ")} |`,
  );
  const report = `# Node WASM worker comparison\n\nMeasured ${metadata.date} at application commit \`${metadata.commit}\` on ${metadata.machine}; Node.js ${metadata.node}.\n\nWarm proof medians in milliseconds:\n\n| Circuit | ${counts.map((count) => (count ? `${count} workers` : "Single session")).join(" | ")} |\n| --- | ${counts.map(() => "---:").join(" | ")} |\n${rows.join("\n")}\n\nEach configuration runs in a fresh process, with three samples after one warm-up and identical captured transaction inputs. The single session keeps a decoded key; the worker path uses Xark's streaming API with a shared worker pool and compressed key bytes. Circuit/key preparation, worker startup, transaction construction, encryption and SPP proofs are excluded. All proofs are verified, checked against the transaction's public hash, and checked for distinct randomness. Worker configurations must invoke both FFT and MSM kernels.\n\nThe fixtures cover create, delivery reclaim, settlement with surplus, and cancellation with both deposits funded. The normal circuit tests pass before measurements begin. These timings show the available threaded API; the application's \`ProgramProver\` still defaults to a single session. Native Rust already uses Rayon.\n\nWASM SHA-256: \`${wasmSha256}\`. Raw samples: \`../target/wasm-thread-benchmark.json\`.\n\nReproduce from \`dvp/typescript\`:\n\n\`\`\`sh\nnpx --no-install tsx bench/threads.ts\n\`\`\`\n`;
  writeFileSync(new URL("../THREAD_BENCHMARK.md", import.meta.url), report);
  execFileSync(fileURLToPath(new URL("../node_modules/.bin/oxfmt", import.meta.url)), [
    "--write",
    fileURLToPath(new URL("../THREAD_BENCHMARK.md", import.meta.url)),
  ]);
}

if (process.argv[2] === "capture") await capture();
else if (process.argv[2] === "measure") await measure(Number(process.argv[3]));
else await main();
