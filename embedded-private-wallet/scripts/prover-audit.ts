import assert from "node:assert/strict";
import { appendFileSync } from "node:fs";
import { ZOLANA_PROVER_URL } from "../src/lib/config.js";

export type ProverAuditEntry = {
  method: string;
  path: string;
  status: number;
};

// Test-only preload. Log metadata; never headers, queries or bodies.
const auditPath = process.env.E2E_AUDIT_PATH;
assert.ok(auditPath, "prover-audit must be started by the devnet E2E runner");
const proverOrigin = new URL(ZOLANA_PROVER_URL).origin;
const originalFetch = globalThis.fetch;
globalThis.fetch = async (input, init) => {
  const request = input instanceof Request ? input : undefined;
  const url = new URL(request?.url ?? String(input));
  if (url.origin !== proverOrigin) return originalFetch(input, init);

  const method = (init?.method ?? request?.method ?? "GET").toUpperCase();
  const headers = new Headers(init?.headers ?? request?.headers);
  const proof = method === "POST" && url.pathname.startsWith("/prove/");
  if (proof) {
    assert.equal(headers.get("zolana-tee"), "v1", "plaintext proof request");
    assert.ok(headers.get("zolana-tee-enc"), "missing encapsulated proof key");
    assert.equal(headers.get("content-type"), "application/octet-stream");
  }
  const response = await originalFetch(input, init);
  if (proof) {
    assert.equal(
      response.headers.get("zolana-tee"),
      "v1",
      "unencrypted proof response",
    );
  }
  const entry: ProverAuditEntry = {
    method,
    path: url.pathname,
    status: response.status,
  };
  appendFileSync(auditPath, `${JSON.stringify(entry)}\n`, { mode: 0o600 });
  return response;
};
