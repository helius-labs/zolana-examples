import { spawn } from "node:child_process";
import { appendFileSync } from "node:fs";
import { join } from "node:path";
import { createInterface } from "node:readline";
import { fileURLToPath } from "node:url";

export const packageRoot = fileURLToPath(new URL("../", import.meta.url));

export function redact(text: string): string {
  const key = process.env.HELIUS_API_KEY;
  return (key ? text.replaceAll(key, "[redacted]") : text).replace(
    /(api-key=)[^\s&"']+/gi,
    "$1[redacted]",
  );
}

export async function startServer(run: string, signal: AbortSignal) {
  const ready = Promise.withResolvers<string>();
  const child = spawn(
    process.execPath,
    [
      "--import",
      "tsx",
      "--import",
      fileURLToPath(new URL("./prover-audit.ts", import.meta.url)),
      "src/index.ts",
    ],
    {
      cwd: packageRoot,
      env: {
        ...process.env,
        PORT: "0",
        DATA_DIR: join(run, "data"),
        E2E_AUDIT_PATH: join(run, "prover-audit.jsonl"),
      },
      stdio: ["ignore", "pipe", "pipe"],
      signal,
    },
  );
  child.once("error", ready.reject);
  child.once("exit", (code, signal) =>
    ready.reject(
      new Error(`Starter exited (${signal ?? code}); see server.log`),
    ),
  );
  for (const output of [child.stdout, child.stderr]) {
    createInterface({ input: output }).on("line", (line) => {
      appendFileSync(join(run, "server.log"), `${redact(line)}\n`, {
        mode: 0o600,
      });
      const match =
        /^Private wallet starter listening on (http:\/\/localhost:\d+)$/.exec(
          line,
        );
      if (output === child.stdout && match) ready.resolve(match[1]);
    });
  }
  async function stop() {
    if (!child.pid || child.exitCode !== null || child.signalCode !== null)
      return;
    const exited = new Promise<void>((resolve) =>
      child.once("close", () => resolve()),
    );
    child.kill("SIGTERM");
    const timer = setTimeout(() => child.kill("SIGKILL"), 5000);
    timer.unref();
    try {
      await exited;
    } finally {
      clearTimeout(timer);
    }
  }
  const timer = setTimeout(
    () => ready.reject(new Error("Starter readiness timeout; see server.log")),
    30_000,
  );
  try {
    return { url: await ready.promise, stop };
  } catch (error) {
    await stop();
    throw error;
  } finally {
    clearTimeout(timer);
  }
}
