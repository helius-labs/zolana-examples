import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdir, mkdtemp, readFile, writeFile } from "node:fs/promises";
import { join, resolve } from "node:path";
import { config as loadEnv } from "dotenv";
import {
  createZolanaClient,
  SPL_TOKEN_2022_PROGRAM_ID,
} from "@heliuslabs/zolana";
import {
  createKeyPairSignerFromBytes,
  generateKeyPairSigner,
  createTransactionMessage,
  setTransactionMessageFeePayerSigner,
  setTransactionMessageLifetimeUsingBlockhash,
  appendTransactionMessageInstructions,
  signTransactionMessageWithSigners,
  getSignatureFromTransaction,
  sendTransactionWithoutConfirmingFactory,
  pipe,
  type Address,
  type Signature,
  type Instruction,
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
import { packageRoot, redact, startServer } from "./starter.js";
import type { ProverAuditEntry } from "./prover-audit.js";

type HistoryEntry = [
  signature: Signature,
  kind: string,
  direction: string,
  amount: string,
];
type HistoryRow = {
  signature: Signature;
  kind: string;
  direction: string;
  amount: string;
  slot: string;
};

loadEnv({ path: join(packageRoot, ".env"), quiet: true });

async function main() {
  const payerPath = process.env.DEVNET_PAYER_KEYPAIR_PATH?.trim();
  assert.ok(payerPath, "Set DEVNET_PAYER_KEYPAIR_PATH in .env");
  const bytes: unknown = JSON.parse(
    await readFile(resolve(packageRoot, payerPath), "utf8"),
  );
  assert.ok(
    Array.isArray(bytes) &&
      bytes.length === 64 &&
      bytes.every((b) => Number.isInteger(b) && b >= 0 && b <= 255),
    "Payer keypair must contain 64 integer bytes",
  );
  const sponsor = await createKeyPairSignerFromBytes(Uint8Array.from(bytes));
  const { SOLANA_RPC_URL, ZOLANA_INDEXER_URL, ZOLANA_PROVER_URL } =
    await import("../src/lib/config.js");
  const client = await createZolanaClient({
    solanaRpcUrl: SOLANA_RPC_URL,
    indexerUrl: ZOLANA_INDEXER_URL,
    proverUrl: ZOLANA_PROVER_URL,
  });

  process.umask(0o077);
  const runs = join(packageRoot, ".data", "devnet-e2e");
  await mkdir(runs, { recursive: true, mode: 0o700 });
  const run = await mkdtemp(join(runs, "run-"));
  console.log(`Devnet E2E results: ${run}`);
  const report = {
    revision:
      spawnSync("git", ["rev-parse", "HEAD"], {
        cwd: packageRoot,
        encoding: "utf8",
        stdio: ["ignore", "pipe", "ignore"],
      }).stdout?.trim() || null,
    startedAt: new Date().toISOString(),
    status: "running",
    checks: [] as string[],
    wallets: [] as Address[],
    transactions: {} as Record<string, Signature>,
    mints: {} as Record<string, Address>,
    failure: null as string | null,
    finishedAt: null as string | null,
  };
  const save = () =>
    writeFile(
      join(run, "results.json"),
      JSON.stringify(report, null, 2) + "\n",
      { mode: 0o600 },
    );
  async function pass(name: string) {
    report.checks.push(name);
    await save();
    console.log(`PASS ${name}`);
  }

  const controller = new AbortController();
  const { signal } = controller;
  const interrupt = () => controller.abort(new Error("Interrupted"));
  process.once("SIGINT", interrupt);
  process.once("SIGTERM", interrupt);
  let server: Awaited<ReturnType<typeof startServer>> | undefined;

  async function api<T = { signature: Signature }>(
    path: string,
    body?: unknown,
    status = 200,
  ): Promise<T> {
    assert.ok(server);
    const response = await fetch(new URL(path, server.url), {
      method: body === undefined ? "GET" : "POST",
      headers: { "content-type": "application/json" },
      body: body === undefined ? undefined : JSON.stringify(body),
      signal: AbortSignal.any([signal, AbortSignal.timeout(180_000)]),
    });
    const json: unknown = await response.json();
    assert.equal(
      response.status,
      status,
      redact(`${path}: ${JSON.stringify(json)}`),
    );
    return json as T;
  }
  async function record(name: string, signature: Signature) {
    report.transactions[name] = signature;
    await save();
    return signature;
  }
  async function transact(
    name: string,
    owner: Address,
    action: string,
    body: unknown,
  ) {
    return record(
      name,
      (await api(`/wallets/${owner}/${action}`, body)).signature,
    );
  }
  async function send(name: string, instructions: Instruction[]) {
    signal.throwIfAborted();
    const { value: blockhash } = await client.solanaRpc
      .getLatestBlockhash({ commitment: "confirmed" })
      .send();
    const message = pipe(
      createTransactionMessage({ version: 0 }),
      (message) => setTransactionMessageFeePayerSigner(sponsor, message),
      (message) =>
        setTransactionMessageLifetimeUsingBlockhash(blockhash, message),
      (message) => appendTransactionMessageInstructions(instructions, message),
    );
    const tx = await signTransactionMessageWithSigners(message, {
      abortSignal: signal,
    });
    await sendTransactionWithoutConfirmingFactory({ rpc: client.solanaRpc })(
      tx,
      { commitment: "confirmed", abortSignal: signal },
    );
    const signature = await record(name, getSignatureFromTransaction(tx));
    await client.confirmTransaction(signature);
    return signature;
  }
  async function balance(owner: Address, amount: string) {
    assert.equal(
      (await api<{ privateLamports: string }>(`/wallets/${owner}`))
        .privateLamports,
      amount,
    );
  }
  async function history(owner: Address, expected: HistoryEntry[]) {
    const { transactions } = await api<{ transactions: HistoryRow[] }>(
      `/wallets/${owner}/transactions`,
    );
    assert.deepEqual(
      transactions.map(({ signature, kind, direction, amount }) => [
        signature,
        kind,
        direction,
        amount,
      ]),
      expected,
    );
    for (let i = 1; i < transactions.length; i++) {
      assert.ok(
        BigInt(transactions[i - 1].slot) >= BigInt(transactions[i].slot),
      );
    }
  }

  try {
    await save();
    assert.equal(
      await client.solanaRpc.getGenesisHash().send(),
      "EtWTRABZaYq6iMfeYKouRu166VU2xqa1wcaWoxPkrZBG",
      "Expected Solana devnet",
    );
    assert.ok(
      (await client.getBalance(sponsor.address)) >= 200_000_000n,
      "Fund the devnet payer with at least 0.2 SOL",
    );
    server = await startServer(run, signal);
    for (let i = 0; i < 2; i++) {
      const { address } = await api<{ address: Address }>("/wallets", {}, 201);
      report.wallets.push(address);
      await save();
    }
    const [a, b] = report.wallets;
    await pass("create wallets");

    for (const [owner, name] of [
      [a, "a"],
      [b, "b"],
    ] as const) {
      await send(`${name}Funding`, [
        getTransferSolInstruction({
          source: sponsor,
          destination: owner,
          amount: 70_000_000n,
        }),
      ]);
      await balance(owner, "0");
    }
    await pass("fund wallets");

    const aDeposit = await transact("aDeposit", a, "deposit", {
      lamports: "10000000",
    });
    await balance(a, "10000000");
    const bDeposit = await transact("bDeposit", b, "deposit", {
      lamports: "5000000",
    });
    await balance(b, "5000000");
    await pass("deposits");

    const transfer = await transact("transfer", a, "transfer", {
      recipient: b,
      lamports: "3000000",
    });
    await balance(a, "7000000");
    await balance(b, "8000000");
    await pass("private transfer");

    const withdrawal = await transact("withdrawal", a, "withdraw", {
      lamports: "3000000",
    });
    await balance(a, "4000000");
    await balance(b, "8000000");
    await pass("withdrawal");

    const aHistory: HistoryEntry[] = [
      [withdrawal, "publicWithdrawal", "outbound", "3000000"],
      [transfer, "privateTransfer", "outbound", "3000000"],
      [aDeposit, "deposit", "inbound", "10000000"],
    ];
    const bHistory: HistoryEntry[] = [
      [transfer, "privateTransfer", "inbound", "3000000"],
      [bDeposit, "deposit", "inbound", "5000000"],
    ];
    await history(a, aHistory);
    await history(b, bHistory);
    await pass("history");

    const beforeA = await client.getBalance(a);
    const beforeB = await client.getBalance(b);
    await transact("publicSol", a, "send", {
      recipient: b,
      lamports: "1000000",
    });
    assert.equal(await client.getBalance(b), beforeB + 1_000_000n);
    assert.ok((await client.getBalance(a)) <= beforeA - 1_000_000n);
    await pass("public SOL");

    const unregistered = await generateKeyPairSigner();
    await api(
      `/wallets/${a}/transfer`,
      { recipient: unregistered.address, lamports: "1000" },
      400,
    );
    await api(`/wallets/${a}/deposit`, { lamports: "0" }, 400);
    await api(
      `/wallets/${a}/deposit`,
      { lamports: Number.MAX_SAFE_INTEGER + 1 },
      400,
    );
    await api(
      `/wallets/${a}/send`,
      { recipient: "bad-address", lamports: "1000" },
      400,
    );
    await api("/wallets/not-an-address", undefined, 404);
    await pass("invalid requests");

    for (const [name, tokenProgram] of [
      ["spl", TOKEN_PROGRAM_ADDRESS],
      ["token2022", SPL_TOKEN_2022_PROGRAM_ID],
    ] as const) {
      const mint = await generateKeyPairSigner();
      const [[source], [destination]] = await Promise.all(
        [a, b].map((owner) =>
          findAssociatedTokenPda({ mint: mint.address, owner, tokenProgram }),
        ),
      );
      const rent = await client.solanaRpc
        .getMinimumBalanceForRentExemption(BigInt(getMintSize()))
        .send();
      const config = { programAddress: tokenProgram };
      report.mints[name] = mint.address;
      await send(`${name}Setup`, [
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
          config,
        ),
        getCreateAssociatedTokenIdempotentInstruction({
          payer: sponsor,
          ata: source,
          owner: a,
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
          config,
        ),
      ]);
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
      for (const [index, amount, received] of [
        [0, 250, 250n],
        [1, 100, 350n],
      ] as const) {
        await transact(`${name}Send${index}`, a, "send", {
          recipient: b,
          mint: mint.address,
          amount: String(amount),
        });
        const [from, to] = await Promise.all(
          [source, destination].map((account) =>
            fetchToken(client.solanaRpc, account, { commitment: "confirmed" }),
          ),
        );
        assert.equal(from.programAddress, tokenProgram);
        assert.equal(to.programAddress, tokenProgram);
        assert.equal(to.data.owner, b);
        assert.equal(from.data.amount, 1000n - received);
        assert.equal(to.data.amount, received);
      }
      await pass(name);
    }

    await server.stop();
    server = await startServer(run, signal);
    await balance(a, "4000000");
    await balance(b, "8000000");
    await history(a, aHistory);
    await history(b, bHistory);
    await pass("reopen wallets");

    const audit = (await readFile(join(run, "prover-audit.jsonl"), "utf8"))
      .trim()
      .split("\n")
      .map((line): ProverAuditEntry => JSON.parse(line));
    assert.ok(
      audit.filter(
        (entry) =>
          entry.method === "GET" &&
          entry.path === "/tee/v1/attestation" &&
          entry.status === 200,
      ).length >= 2,
    );
    const proofs = audit.filter(
      (entry) => entry.method === "POST" && entry.path.startsWith("/prove/"),
    );
    assert.equal(proofs.length, 2);
    assert.ok(proofs.every((entry) => entry.status === 200));
    await pass("TEE encryption");
    report.status = "passed";
  } catch (error) {
    report.status = signal.aborted ? "interrupted" : "failed";
    report.failure = redact(String(error));
    throw error;
  } finally {
    try {
      await server?.stop();
    } finally {
      report.finishedAt = new Date().toISOString();
      await save();
      process.removeListener("SIGINT", interrupt);
      process.removeListener("SIGTERM", interrupt);
    }
  }
}

await main().catch((error) => {
  console.error(redact(String(error)));
  process.exitCode = 1;
});
