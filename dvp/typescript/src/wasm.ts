import { createRequire } from "node:module";
import { readFileSync } from "node:fs";
import { createHash } from "node:crypto";

export type * from "../../target/zk/dvp-program/wasm/dvp_program.js";
export const wasm: typeof import("../../target/zk/dvp-program/wasm/dvp_program.js") = createRequire(
  import.meta.url,
)("../../target/zk/dvp-program/wasm/dvp_program.js");

export function provingKey(name: "create_dvp" | "reclaim_dvp" | "settle_dvp" | "abort_dvp") {
  const manifest = readFileSync(new URL("../../dvp-keys.CHECKSUM", import.meta.url), "utf8");
  const digest = manifest
    .split("\n")
    .map((line) => line.trim().split(/\s+/))
    .find((parts) => parts[1] === `${name}.pk`)?.[0];
  if (!digest || !/^[0-9a-f]{64}$/.test(digest)) throw new Error(`No trusted checksum for ${name}`);
  const key = readFileSync(new URL(`../../target/zk/dvp-program/${name}.pk`, import.meta.url));
  if (createHash("sha256").update(key).digest("hex") !== digest)
    throw new Error("DvP proving key checksum mismatch");
  return { key, checksum: Uint8Array.from(Buffer.from(digest, "hex")) };
}
