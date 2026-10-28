import { readFileSync } from "node:fs";
import { createHash } from "node:crypto";
import * as runtime from "../../target/zk/dvp-program/wasm/dvp_program.js";
import type {
  ProgramTransaction,
  ProgramProof,
} from "../../target/zk/dvp-program/wasm/dvp_program.js";

export { runtime as wasm };
export type * from "../../target/zk/dvp-program/wasm/dvp_program.js";
export type CircuitName = "create_dvp" | "reclaim_dvp" | "settle_dvp" | "abort_dvp";

export function circuitArtifacts(name: CircuitName) {
  const directory = new URL(`../../target/zk/dvp-program/${name}/`, import.meta.url);
  const manifest = readFileSync(new URL("../../dvp-keys.CHECKSUM", import.meta.url), "utf8");
  const checkedArtifact = (filename: "pk.bin" | "vk.bin" | "circuit.xbc") => {
    const digest = manifest
      .split("\n")
      .map((line) => line.trim().split(/\s+/))
      .find((parts) => parts[1] === `${name}/${filename}`)?.[0];
    if (!digest || !/^[0-9a-f]{64}$/.test(digest))
      throw new Error(`No trusted checksum for ${name}/${filename}`);
    const artifact = readFileSync(new URL(filename, directory));
    if (createHash("sha256").update(artifact).digest("hex") !== digest)
      throw new Error(`DvP artifact checksum mismatch: ${name}/${filename}`);
    return artifact;
  };
  return {
    provingKey: checkedArtifact("pk.bin"),
    verifyingKey: checkedArtifact("vk.bin"),
    circuit: checkedArtifact("circuit.xbc"),
    sources: readFileSync(new URL("sources.json", directory), "utf8"),
  };
}

/** Keeps a decoded Xark circuit, proving key and verifier for repeated proofs. */
export class ProgramProver {
  readonly #session: InstanceType<typeof runtime.ProvingSession>;
  readonly #verifier: InstanceType<typeof runtime.VerifyingKey>;
  #disposed = false;

  constructor(readonly name: CircuitName) {
    const artifacts = circuitArtifacts(name);
    this.#verifier = new runtime.VerifyingKey(artifacts.verifyingKey);
    try {
      this.#session = new runtime.ProvingSession(
        artifacts.circuit,
        artifacts.provingKey,
        artifacts.sources,
      );
    } catch (error) {
      this.#verifier.free();
      throw error;
    }
  }

  prove(transaction: Pick<ProgramTransaction, "proofInputs" | "publicHash">): ProgramProof {
    if (this.#disposed) throw new Error("DvP prover is disposed");
    const bundle = this.#session.prove(transaction.proofInputs);
    try {
      if (!this.#verifier.verify(bundle.proof, bundle.publicInputs))
        throw new Error("DvP proof verification failed");
      return runtime.programProof(bundle.proof, bundle.publicInputs, transaction.publicHash);
    } finally {
      bundle.free();
    }
  }

  free(): void {
    if (this.#disposed) return;
    this.#disposed = true;
    this.#session.free();
    this.#verifier.free();
  }
}
