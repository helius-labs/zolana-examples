import assert from "node:assert/strict";
import { execFileSync, spawn, type ChildProcess } from "node:child_process";
import { appendFileSync } from "node:fs";
import { mkdir, mkdtemp, readFile, writeFile } from "node:fs/promises";
import { createServer } from "node:net";
import { once } from "node:events";
import { join, resolve } from "node:path";
import { createInterface } from "node:readline";
import { setTimeout as delay } from "node:timers/promises";
import { fileURLToPath } from "node:url";
import { config as loadEnv } from "dotenv";
import { createZolanaClient, initializePoseidon } from "@heliuslabs/zolana";
import {
  address,
  createKeyPairSignerFromBytes,
  generateKeyPairSigner,
  createTransactionMessage,
  setTransactionMessageFeePayerSigner,
  setTransactionMessageLifetimeUsingBlockhash,
  appendTransactionMessageInstructions,
  compileTransaction,
  signTransactionWithSigners,
  assertIsFullySignedTransaction,
  getSignatureFromTransaction,
  sendTransactionWithoutConfirmingFactory,
  type Address,
  type Signature,
  type Instruction,
  type TransactionPartialSigner,
} from "@solana/kit";
import {
  getCreateAccountInstruction,
  getTransferSolInstruction,
} from "@solana-program/system";
import {
  TOKEN_PROGRAM_ADDRESS,
  getMintSize,
  getInitializeMint2Instruction,
  findAssociatedTokenPda,
  getCreateAssociatedTokenIdempotentInstruction,
  getMintToInstruction,
  fetchToken,
} from "@solana-program/token";
import type { ProverAuditEntry } from "./prover-audit.js";

type WalletName = "a" | "b";
type WalletResponse = {
  address: Address;
  privateLamports: string;
  publicLamports: string;
};
type SignatureResponse = { signature: Signature };
type HistoryRow = {
  signature: Signature;
  kind: string;
  direction: string;
  amount: string;
  slot: string;
};
type HistoryValue = Omit<HistoryRow, "slot">;
type Evidence = {
  revision: string | null;
  startedAt: string;
  finishedAt?: string;
  status: "running" | "passed" | "failed" | "interrupted";
  checks: { name: string; details?: unknown }[];
  wallets: Partial<Record<WalletName, Address>>;
  transactions: Record<string, Signature>;
  mints: Record<string, Address>;
  failure?: string;
};

const packageRoot = fileURLToPath(new URL("../", import.meta.url));
loadEnv({ path: join(packageRoot, ".env"), quiet: true });

function redact(text: string): string {
  const key = process.env.HELIUS_API_KEY;
  return (key ? text.replaceAll(key, "[redacted]") : text).replace(
    /(api-key=)[^\s&"']+/gi,
    "$1[redacted]",
  );
}

async function main(): Promise<void> {
  // Validate explicit input before creating wallets or sending any transaction.
  const payerPath = process.env.DEVNET_PAYER_KEYPAIR_PATH?.trim();
  assert.ok(
    payerPath,
    "Set DEVNET_PAYER_KEYPAIR_PATH in .env to a funded devnet-only Solana JSON keypair",
  );
  const bytes: unknown = JSON.parse(
    await readFile(resolve(packageRoot, payerPath), "utf8"),
  );
  assert.ok(
    Array.isArray(bytes) &&
      bytes.length === 64 &&
      bytes.every(
        (b: unknown) =>
          typeof b === "number" && Number.isInteger(b) && b >= 0 && b <= 255,
      ),
    "DEVNET_PAYER_KEYPAIR_PATH must contain 64 integer bytes",
  );
  const sponsor = await createKeyPairSignerFromBytes(Uint8Array.from(bytes));
  const { SOLANA_RPC_URL, ZOLANA_INDEXER_URL, ZOLANA_PROVER_URL } =
    await import("../src/lib/config.js");
  process.umask(0o077);
  const runs = join(packageRoot, ".data", "devnet-e2e");
  await mkdir(runs, { recursive: true, mode: 0o700 });
  const run = await mkdtemp(join(runs, "run-"));
  console.log(`Devnet E2E results: ${run}`);
  let revision: string | null = null;
  try {
    revision = execFileSync("git", ["rev-parse", "HEAD"], {
      cwd: packageRoot,
      encoding: "utf8",
      stdio: ["ignore", "pipe", "ignore"],
    }).trim();
  } catch {
    /* Source archives have no Git metadata. */
  }
  const evidence: Evidence = {
    revision,
    startedAt: new Date().toISOString(),
    status: "running",
    checks: [],
    wallets: {},
    transactions: {},
    mints: {},
  };
  let client: Awaited<ReturnType<typeof createZolanaClient>>;
  let server: ChildProcess | undefined;
  let port: number;
  const save = () =>
    writeFile(
      join(run, "results.json"),
      JSON.stringify(evidence, null, 2) + "\n",
      { mode: 0o600 },
    );
  function wallet(name: WalletName): Address {
    const value = evidence.wallets[name];
    assert.ok(value, `wallet ${name} has not been created`);
    return value;
  }
  async function pass(name: string, details?: unknown): Promise<void> {
    evidence.checks.push({ name, details });
    await save();
    console.log(`PASS ${name}`);
  }

  async function start(): Promise<void> {
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
          PORT: String(port),
          DATA_DIR: join(run, "data"),
          E2E_AUDIT_PATH: join(run, "prover-audit.jsonl"),
        },
        stdio: ["ignore", "pipe", "pipe"],
      },
    );
    server = child;
    let spawnError: Error | undefined;
    child.on("error", (error) => {
      spawnError = error;
    });
    for (const output of [child.stdout, child.stderr]) {
      createInterface({ input: output }).on("line", (line) =>
        appendFileSync(join(run, "server.log"), redact(line) + "\n", {
          mode: 0o600,
        }),
      );
    }
    for (let attempt = 0; attempt < 100; attempt++) {
      if (spawnError) throw spawnError;
      assert.ok(
        child.exitCode === null && child.signalCode === null,
        "starter exited before becoming ready; see server.log",
      );
      try {
        if (
          (
            await fetch(`http://127.0.0.1:${port}/wallets/not-an-address`, {
              signal: AbortSignal.timeout(1000),
            })
          ).status === 404
        )
          return;
      } catch {
        /* Wait for the server to bind. */
      }
      await delay(300);
    }
    throw new Error("starter readiness timeout; see server.log");
  }
  async function stop(): Promise<void> {
    const child = server;
    if (!child?.pid || child.exitCode !== null || child.signalCode !== null)
      return;
    const exited = once(child, "exit");
    child.kill("SIGTERM");
    const timer = setTimeout(() => child.kill("SIGKILL"), 5000);
    timer.unref();
    try {
      await exited;
    } finally {
      clearTimeout(timer);
    }
  }
  async function api<T = SignatureResponse>(
    path: string,
    body?: unknown,
    expected = 200,
  ): Promise<T> {
    const response = await fetch(`http://127.0.0.1:${port}${path}`, {
      method: body === undefined ? "GET" : "POST",
      ...(body === undefined
        ? {}
        : {
            headers: { "content-type": "application/json" },
            body: JSON.stringify(body),
          }),
      signal: AbortSignal.timeout(180_000),
    });
    const json: unknown = await response.json();
    assert.equal(
      response.status,
      expected,
      redact(`${path}: ${JSON.stringify(json)}`),
    );
    return json as T;
  }
  async function send(
    instructions: Instruction[],
    extra: TransactionPartialSigner[] = [],
  ): Promise<Signature> {
    const { value: blockhash } = await client.solanaRpc
      .getLatestBlockhash({ commitment: "confirmed" })
      .send();
    const message = appendTransactionMessageInstructions(
      instructions,
      setTransactionMessageLifetimeUsingBlockhash(
        blockhash,
        setTransactionMessageFeePayerSigner(
          sponsor,
          createTransactionMessage({ version: 0 }),
        ),
      ),
    );
    const tx = await signTransactionWithSigners(
      [sponsor, ...extra],
      compileTransaction(message),
    );
    assertIsFullySignedTransaction(tx);
    await sendTransactionWithoutConfirmingFactory({ rpc: client.solanaRpc })(
      tx,
      { commitment: "confirmed" },
    );
    const signature = getSignatureFromTransaction(tx);
    await client.confirmTransaction(signature);
    return signature;
  }
  async function balance(who: WalletName, amount: string): Promise<void> {
    assert.equal(
      (await api<WalletResponse>(`/wallets/${wallet(who)}`)).privateLamports,
      amount,
    );
  }
  async function history(
    who: WalletName,
    expected: HistoryValue[],
  ): Promise<void> {
    const rows = (
      await api<{ transactions: HistoryRow[] }>(
        `/wallets/${wallet(who)}/transactions`,
      )
    ).transactions;
    assert.deepEqual(
      rows.map(({ signature, kind, direction, amount }) => ({
        signature,
        kind,
        direction,
        amount,
      })),
      expected,
    );
    for (let i = 1; i < rows.length; i++) {
      assert.ok(BigInt(rows[i - 1].slot) >= BigInt(rows[i].slot));
    }
  }
  const interrupt = () => {
    evidence.status = "interrupted";
    evidence.finishedAt = new Date().toISOString();
    void save()
      .finally(stop)
      .finally(() => process.exit(130));
  };
  process.once("SIGINT", interrupt);
  process.once("SIGTERM", interrupt);
  try {
    await save();
    await initializePoseidon();
    client = await createZolanaClient({
      solanaRpcUrl: SOLANA_RPC_URL,
      indexerUrl: ZOLANA_INDEXER_URL,
      proverUrl: ZOLANA_PROVER_URL,
      proofDataSource: "client",
    });
    assert.equal(
      await client.solanaRpc.getGenesisHash().send(),
      "EtWTRABZaYq6iMfeYKouRu166VU2xqa1wcaWoxPkrZBG",
      "E2E funding is allowed only on Solana devnet",
    );
    assert.ok(
      (await client.getBalance(sponsor.address)) >= 200_000_000n,
      "Fund the devnet payer with at least 0.2 SOL",
    );
    const listener = createServer();
    listener.listen(0, "127.0.0.1");
    await once(listener, "listening");
    const bound = listener.address();
    assert.ok(bound && typeof bound !== "string");
    port = bound.port;
    await new Promise<void>((resolve, reject) =>
      listener.close((error) => (error ? reject(error) : resolve())),
    );
    await start();
    evidence.wallets.a = address(
      (await api<WalletResponse>("/wallets", {}, 201)).address,
    );
    evidence.wallets.b = address(
      (await api<WalletResponse>("/wallets", {}, 201)).address,
    );
    await pass("create and enroll two fresh wallets", {
      a: wallet("a"),
      b: wallet("b"),
    });
    for (const who of ["a", "b"] as const) {
      evidence.transactions[who + "Funding"] = await send([
        getTransferSolInstruction({
          source: sponsor,
          destination: address(wallet(who)),
          amount: 70_000_000n,
        }),
      ]);
      await balance(who, "0");
    }
    await pass("fund both wallets; zero private balances");
    for (const [who, amount] of [
      ["a", "10000000"],
      ["b", "5000000"],
    ] as const) {
      evidence.transactions[who + "Deposit"] = (
        await api(`/wallets/${wallet(who)}/deposit`, { lamports: amount })
      ).signature;
      await balance(who, amount);
    }
    await pass("register, deposit, immediate exact balances");
    evidence.transactions.transfer = (
      await api(`/wallets/${wallet("a")}/transfer`, {
        recipient: wallet("b"),
        lamports: "3000000",
      })
    ).signature;
    await balance("a", "7000000");
    await balance("b", "8000000");
    await pass("private transfer; immediate sender and recipient balances");
    evidence.transactions.withdrawal = (
      await api(`/wallets/${wallet("a")}/withdraw`, { lamports: "3000000" })
    ).signature;
    await balance("a", "4000000");
    await balance("b", "8000000");
    await pass("withdraw; immediate exact balances");
    const expectedA: HistoryValue[] = [
      {
        signature: evidence.transactions.withdrawal,
        kind: "publicWithdrawal",
        direction: "outbound",
        amount: "3000000",
      },
      {
        signature: evidence.transactions.transfer,
        kind: "privateTransfer",
        direction: "outbound",
        amount: "3000000",
      },
      {
        signature: evidence.transactions.aDeposit,
        kind: "deposit",
        direction: "inbound",
        amount: "10000000",
      },
    ];
    const expectedB: HistoryValue[] = [
      {
        signature: evidence.transactions.transfer,
        kind: "privateTransfer",
        direction: "inbound",
        amount: "3000000",
      },
      {
        signature: evidence.transactions.bDeposit,
        kind: "deposit",
        direction: "inbound",
        amount: "5000000",
      },
    ];
    await history("a", expectedA);
    await history("b", expectedB);
    await pass(
      "decrypted history: exact signatures, kinds, directions, amounts, ordering",
    );
    const beforeA = await client.getBalance(address(wallet("a")));
    const beforeB = await client.getBalance(address(wallet("b")));
    evidence.transactions.publicSol = (
      await api(`/wallets/${wallet("a")}/send`, {
        recipient: wallet("b"),
        lamports: "1000000",
      })
    ).signature;
    assert.equal(
      await client.getBalance(address(wallet("b"))),
      beforeB + 1_000_000n,
    );
    assert.ok(
      (await client.getBalance(address(wallet("a")))) <= beforeA - 1_000_000n,
    );
    await pass("public SOL transfer; exact recipient delta");
    const fresh = await generateKeyPairSigner();
    await api(
      `/wallets/${wallet("a")}/transfer`,
      { recipient: fresh.address, lamports: "1000" },
      400,
    );
    await api(`/wallets/${wallet("a")}/deposit`, { lamports: "0" }, 400);
    await api(
      `/wallets/${wallet("a")}/send`,
      { recipient: "bad-address", lamports: "1000" },
      400,
    );
    await api("/wallets/not-an-address", undefined, 404);
    await pass(
      "unregistered recipient, invalid amount/address, missing wallet rejected",
    );
    for (const [label, tokenProgram] of [
      ["spl", TOKEN_PROGRAM_ADDRESS],
      ["token2022", address("TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb")],
    ] as const) {
      const mint = await generateKeyPairSigner();
      const [source] = await findAssociatedTokenPda({
        mint: mint.address,
        owner: address(wallet("a")),
        tokenProgram,
      });
      const [destination] = await findAssociatedTokenPda({
        mint: mint.address,
        owner: address(wallet("b")),
        tokenProgram,
      });
      const rent = await client.solanaRpc
        .getMinimumBalanceForRentExemption(BigInt(getMintSize()))
        .send();
      evidence.transactions[label + "Setup"] = await send(
        [
          getCreateAccountInstruction({
            payer: sponsor,
            newAccount: mint,
            lamports: rent,
            space: getMintSize(),
            programAddress: tokenProgram,
          }),
          getInitializeMint2Instruction(
            {
              mint: mint.address,
              decimals: 6,
              mintAuthority: sponsor.address,
              freezeAuthority: null,
            },
            { programAddress: tokenProgram },
          ),
          getCreateAssociatedTokenIdempotentInstruction({
            payer: sponsor,
            ata: source,
            owner: address(wallet("a")),
            mint: mint.address,
            tokenProgram,
          }),
          getMintToInstruction(
            {
              mint: mint.address,
              token: source,
              mintAuthority: sponsor,
              amount: 1000n,
            },
            { programAddress: tokenProgram },
          ),
        ],
        [mint],
      );
      evidence.mints[label] = mint.address;
      await save();
      assert.equal(
        (
          await client.solanaRpc
            .getAccountInfo(destination, {
              encoding: "base64",
              commitment: "confirmed",
            })
            .send()
        ).value,
        null,
      );
      for (const [i, amount] of [
        [0, 250],
        [1, 100],
      ]) {
        evidence.transactions[label + "Send" + i] = (
          await api(`/wallets/${wallet("a")}/send`, {
            recipient: wallet("b"),
            mint: mint.address,
            amount: String(amount),
          })
        ).signature;
        const expected = i === 0 ? 250n : 350n;
        const to = await fetchToken(client.solanaRpc, destination, {
          commitment: "confirmed",
        });
        const from = await fetchToken(client.solanaRpc, source, {
          commitment: "confirmed",
        });
        assert.equal(to.programAddress, tokenProgram);
        assert.equal(to.data.owner, wallet("b"));
        assert.equal(to.data.amount, expected);
        assert.equal(from.data.amount, 1000n - expected);
      }
      await pass(label + " sends create then reuse recipient ATA", {
        mint: mint.address,
        sender: "650",
        recipient: "350",
      });
    }
    await stop();
    await start();
    await balance("a", "4000000");
    await balance("b", "8000000");
    await history("a", expectedA);
    await history("b", expectedB);
    await pass("restart, reopen sealed wallets, exact balances and history");
    const audit = (await readFile(join(run, "prover-audit.jsonl"), "utf8"))
      .trim()
      .split("\n")
      .map((line): ProverAuditEntry => JSON.parse(line));
    assert.ok(
      audit.filter((x) => x.path === "/tee/v1/attestation").length >= 2,
    );
    const proofs = audit.filter((x) => x.method === "POST");
    assert.equal(proofs.length, 2);
    assert.ok(
      proofs.every(
        (x) => x.encrypted && x.responseEncrypted && x.status === 200,
      ),
    );
    await pass(
      "live TEE attestation; encrypted proof requests and responses",
      audit,
    );
    evidence.status = "passed";
    console.log("ALL DEVNET E2E CHECKS PASSED");
  } catch (error) {
    evidence.status = "failed";
    evidence.failure = redact(String(error));
    throw error;
  } finally {
    await stop();
    evidence.finishedAt = new Date().toISOString();
    await save();
    process.removeListener("SIGINT", interrupt);
    process.removeListener("SIGTERM", interrupt);
  }
}

await main().catch((error) => {
  console.error(redact(String(error)));
  process.exitCode = 1;
});
