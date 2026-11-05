import "dotenv/config";
import assert from "node:assert/strict";
import { spawn, type ChildProcess } from "node:child_process";
import { generateKeyPairSync } from "node:crypto";
import { once } from "node:events";
import { setTimeout as delay } from "node:timers/promises";
import { fileURLToPath } from "node:url";
import { SOL_MINT, Wallet } from "@heliuslabs/zolana";
import { LocalKeys } from "@heliuslabs/zolana/client";
import {
  getAssociatedTokenAddress,
  getSolInterfaceAddress,
  getSplAssetRegistryAddress,
  getSplAssetVaultAddress,
} from "@heliuslabs/zolana/addresses";
import {
  getCreateAssociatedTokenAccountInstructionAsync,
  getRegisterInstructionAsync,
} from "@heliuslabs/zolana/instructions";
import {
  decodeSplAssetRegistry,
  SHIELDED_POOL_PROGRAM_ID,
} from "@heliuslabs/zolana/interface";
import {
  ed25519DerivationMessage,
  type Bytes32,
} from "@heliuslabs/zolana/keypair";
import {
  resolveRegisteredAddress,
  syncWallet,
} from "@heliuslabs/zolana/wallet";
import { getCreateAccountInstruction } from "@solana-program/system";
import {
  TOKEN_PROGRAM_ADDRESS,
  getInitializeMint2Instruction,
  getMintSize,
  getMintToInstruction,
} from "@solana-program/token";
import {
  address,
  createNoopSigner,
  generateKeyPairSigner,
  getAddressEncoder,
  getBase58Encoder,
  getProgramDerivedAddress,
  getTransactionDecoder,
  getTransactionEncoder,
  signature,
  partiallySignTransactionWithSigners,
  type Address,
  type Instruction,
  type TransactionPartialSigner,
} from "@solana/kit";
import {
  getClient,
  deriveKeys,
  encodeTransaction,
  privy,
  TREASURY_AUTHORIZATION_KEY,
  type SolanaWallet,
} from "../src/confidential.mjs";

const cwd = fileURLToPath(new URL("../", import.meta.url));
const senderId = process.env.SENDER_WALLET_ID!;
const recipientId = process.env.RECIPIENT_WALLET_ID!;
const client = await getClient();
let server: ChildProcess | undefined;
let senderKeys: LocalKeys | undefined;
let recipientKeys: LocalKeys | undefined;
let scenario = "base";

async function stopServer() {
  if (!server) return;
  const child = server;
  server = undefined;
  if (child.exitCode !== null || child.signalCode !== null) return;
  const exited = once(child, "exit");
  const timer = setTimeout(() => child.kill("SIGKILL"), 5_000);
  try {
    child.kill("SIGTERM");
    await exited;
  } finally {
    clearTimeout(timer);
  }
}

async function startServer(walletId: string) {
  await stopServer();
  const child = spawn(process.execPath, ["--import", "tsx", "src/index.ts"], {
    cwd,
    env: { ...process.env, SENDER_WALLET_ID: walletId },
    stdio: ["ignore", "pipe", "pipe"],
  });
  server = child;
  child.stderr?.resume();
  await new Promise<void>((resolve, reject) => {
    const timer = setTimeout(
      () => reject(new Error("Express startup timed out")),
      30_000,
    );
    child.once("error", () => {
      clearTimeout(timer);
      reject(new Error("Express failed to start"));
    });
    child.once("exit", () => {
      clearTimeout(timer);
      reject(new Error("Express exited before listening"));
    });
    child.stdout?.on("data", (chunk: Buffer) => {
      if (chunk.toString().includes("Example app listening on port 3300")) {
        clearTimeout(timer);
        resolve();
      }
    });
  });
}

async function confirmedTransaction(hash: string) {
  for (let attempt = 0; attempt < 20; attempt++) {
    const tx = await client.solanaRpc
      .getTransaction(signature(hash), {
        encoding: "json",
        commitment: "confirmed",
        maxSupportedTransactionVersion: 0,
      })
      .send();
    if (tx) {
      assert.ok(tx.meta);
      assert.equal(tx.meta.err, null);
      return tx;
    }
    await delay(250);
  }
  throw new Error("Confirmed transaction metadata was unavailable");
}

async function request(
  operation: string,
  body: object,
  solVaultDelta?: bigint,
) {
  const started = performance.now();
  const response = await fetch(
    "http://localhost:3300/transactions/" + operation,
    {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    },
  );
  assert.equal(response.status, 200, operation + " endpoint failed");
  const result = (await response.json()) as { hash: string };
  const elapsedMs = performance.now() - started;
  assert.equal(typeof result.hash, "string");
  const tx = await confirmedTransaction(result.hash);
  const wire = await client.solanaRpc
    .getTransaction(signature(result.hash), {
      encoding: "base64",
      commitment: "confirmed",
      maxSupportedTransactionVersion: 0,
    })
    .send();
  assert.ok(wire);
  assert.ok(tx.meta);
  assert.notEqual(tx.meta.computeUnitsConsumed, undefined);
  if (solVaultDelta !== undefined) {
    assert.ok(tx.meta);
    const accounts = [
      ...tx.transaction.message.accountKeys,
      ...(tx.meta.loadedAddresses?.writable ?? []),
      ...(tx.meta.loadedAddresses?.readonly ?? []),
    ];
    const index = accounts.indexOf(getSolInterfaceAddress());
    assert.ok(index >= 0, "SOL interface missing");
    assert.equal(
      tx.meta.postBalances[index]! - tx.meta.preBalances[index]!,
      solVaultDelta,
    );
  }
  console.log(
    JSON.stringify({
      operation,
      ...body,
      scenario,
      hash: result.hash,
      elapsedMs: Number(elapsedMs.toFixed(1)),
      computeUnits: Number(tx.meta.computeUnitsConsumed),
      feeLamports: tx.meta.fee.toString(),
      instructionDataBytes: tx.transaction.message.instructions.reduce(
        (sum, instruction) =>
          sum + getBase58Encoder().encode(instruction.data).byteLength,
        0,
      ),
      accounts:
        tx.transaction.message.accountKeys.length +
        (tx.meta.loadedAddresses?.writable.length ?? 0) +
        (tx.meta.loadedAddresses?.readonly.length ?? 0),
      transactionBytes: Buffer.from(wire.transaction[0], "base64").byteLength,
    }),
  );
  return tx.slot;
}

async function sendFixture(
  wallet: SolanaWallet,
  instructions: readonly Instruction[],
  signers: readonly TransactionPartialSigner[] = [],
) {
  let transaction = await encodeTransaction(
    client,
    address(wallet.address),
    instructions,
  );
  if (signers.length) {
    const partial = await partiallySignTransactionWithSigners(
      signers,
      getTransactionDecoder().decode(transaction),
    );
    transaction = new Uint8Array(getTransactionEncoder().encode(partial));
  }
  const result = await privy
    .wallets()
    .solana()
    .signAndSendTransaction(wallet.id, {
      caip2: "solana:EtWTRABZaYq6iMfeYKouRu166VU2xqa1",
      transaction,
      authorization_context: {
        authorization_private_keys: [TREASURY_AUTHORIZATION_KEY],
      },
    });
  await client.confirmTransaction(signature(result.hash));
  await confirmedTransaction(result.hash);
  console.log(JSON.stringify({ setup: true, hash: result.hash }));
}

async function register(wallet: SolanaWallet, keys: LocalKeys) {
  const owner = address(wallet.address);
  const identity = keys.address();
  const existing = await resolveRegisteredAddress({ rpc: client, owner });
  if (existing) {
    assert.deepEqual(
      existing.address.nullifierPublicKey,
      identity.nullifierPublicKey,
    );
    assert.ok(
      existing.address.viewingPublicKey.equals(identity.viewingPublicKey),
    );
    return;
  }
  await sendFixture(wallet, [
    await getRegisterInstructionAsync({
      owner,
      nullifierPublicKey: identity.nullifierPublicKey,
      viewingPublicKey: identity.viewingPublicKey.toBytes(),
    }),
  ]);
}

async function setupToken(wallet: SolanaWallet) {
  const owner = address(wallet.address);
  const payer = createNoopSigner(owner);
  const mint = await generateKeyPairSigner();
  const token = await getAssociatedTokenAddress(
    owner,
    mint.address,
    TOKEN_PROGRAM_ADDRESS,
  );
  const rent = await client.solanaRpc
    .getMinimumBalanceForRentExemption(BigInt(getMintSize()))
    .send();
  await sendFixture(
    wallet,
    [
      getCreateAccountInstruction({
        payer,
        newAccount: mint,
        lamports: rent,
        space: getMintSize(),
        programAddress: TOKEN_PROGRAM_ADDRESS,
      }),
      getInitializeMint2Instruction({
        mint: mint.address,
        decimals: 9,
        mintAuthority: owner,
        freezeAuthority: null,
      }),
      await getCreateAssociatedTokenAccountInstructionAsync({
        payer: owner,
        owner,
        mint: mint.address,
        tokenProgram: TOKEN_PROGRAM_ADDRESS,
      }),
      getMintToInstruction({
        mint: mint.address,
        token,
        mintAuthority: payer,
        amount: 1_000_000_000n,
      }),
    ],
    [mint],
  );
  return mint.address;
}

async function privateBalances(mint: Address, slot: bigint) {
  const sender = new Wallet({ identity: senderKeys!.address() });
  const recipient = new Wallet({ identity: recipientKeys!.address() });
  if (mint !== SOL_MINT) {
    const account = await client.getAccount(
      await getSplAssetRegistryAddress(mint),
    );
    assert.ok(account);
    const registration = decodeSplAssetRegistry(account.data);
    sender.registerAsset(registration.assetId, registration.mint);
    recipient.registerAsset(registration.assetId, registration.mint);
  }
  await Promise.all([
    syncWallet({
      wallet: sender,
      keys: senderKeys!,
      client,
      config: { requireSlot: slot },
    }),
    syncWallet({
      wallet: recipient,
      keys: recipientKeys!,
      client,
      config: { requireSlot: slot },
    }),
  ]);
  return [sender.balance(mint).amount, recipient.balance(mint).amount];
}

async function publicTokens(owner: Address, mint: Address) {
  const ata = await getAssociatedTokenAddress(
    owner,
    mint,
    TOKEN_PROGRAM_ADDRESS,
  );
  if (!(await client.getAccount(ata))) return 0n;
  return BigInt(
    (await client.solanaRpc.getTokenAccountBalance(ata).send()).value.amount,
  );
}

async function authorizationMustFail(action: () => Promise<unknown>) {
  let status: number | undefined;
  try {
    await action();
  } catch (error) {
    status = Number((error as { status?: unknown }).status);
  }
  assert.ok(
    status === 401 || status === 403,
    "Expected Privy authorization rejection",
  );
}

try {
  assert.ok(senderId && recipientId && TREASURY_AUTHORIZATION_KEY);
  assert.notEqual(senderId, recipientId);
  const sender = await privy.wallets().get(senderId);
  const recipient = await privy.wallets().get(recipientId);
  assert.equal(sender.chain_type, "solana");
  assert.equal(recipient.chain_type, "solana");
  const senderAddress = address(sender.address);
  const recipientAddress = address(recipient.address);
  senderKeys = await deriveKeys(client, sender);
  recipientKeys = await deriveKeys(client, recipient);

  const message = ed25519DerivationMessage(
    new Uint8Array(getAddressEncoder().encode(senderAddress)) as Bytes32,
  );
  await authorizationMustFail(() =>
    privy.wallets().solana().signMessage(senderId, { message }),
  );
  const wrongKey = generateKeyPairSync("ec", { namedCurve: "prime256v1" })
    .privateKey.export({ type: "pkcs8", format: "der" })
    .toString("base64");
  await authorizationMustFail(() =>
    privy
      .wallets()
      .solana()
      .signMessage(senderId, {
        message,
        authorization_context: { authorization_private_keys: [wrongKey] },
      }),
  );

  await register(sender, senderKeys);
  await register(recipient, recipientKeys);
  const slot = await client.solanaRpc
    .getSlot({ commitment: "confirmed" })
    .send();
  assert.deepEqual(await privateBalances(SOL_MINT, slot), [0n, 0n]);
  const mint = await setupToken(sender);
  let testedUnregistered = false;

  for (const [asset, deposits, transferAmount] of [
    [SOL_MINT, [10_000_000n], 3_000_000n],
    [mint, [1_000_000_000n], 300_000_000n],
    [SOL_MINT, [3_000_000n, 3_000_000n], 4_000_000n],
    [mint, [300_000_000n, 300_000_000n], 400_000_000n],
  ] as const) {
    scenario = deposits.length === 1 ? "base" : "multi-input";
    const total = deposits.reduce<bigint>((sum, amount) => sum + amount, 0n);
    const beforeSender =
      asset === SOL_MINT ? 0n : await publicTokens(senderAddress, asset);
    const beforeRecipient =
      asset === SOL_MINT ? 0n : await publicTokens(recipientAddress, asset);
    await startServer(senderId);
    let lastSlot = slot;
    for (const amount of deposits) {
      lastSlot = await request(
        "deposit",
        { mint: asset, amount: amount.toString() },
        asset === SOL_MINT ? amount : undefined,
      );
    }
    assert.deepEqual(await privateBalances(asset, lastSlot), [total, 0n]);

    if (!testedUnregistered) {
      const unregistered = await generateKeyPairSigner();
      const response = await fetch(
        "http://localhost:3300/transactions/transfer",
        {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({
            recipient: unregistered.address,
            amount: "1",
          }),
        },
      );
      assert.equal(response.status, 500);
      assert.ok(
        (await response.text()).includes("Recipient is not registered"),
      );
      assert.deepEqual(await privateBalances(asset, lastSlot), [total, 0n]);
      testedUnregistered = true;
    }

    lastSlot = await request("transfer", {
      mint: asset,
      recipient: recipient.address,
      amount: transferAmount.toString(),
    });
    assert.deepEqual(await privateBalances(asset, lastSlot), [
      total - transferAmount,
      transferAmount,
    ]);

    await startServer(senderId);
    lastSlot = await request(
      "withdraw",
      {
        mint: asset,
        recipient: sender.address,
        amount: (total - transferAmount).toString(),
      },
      asset === SOL_MINT ? -(total - transferAmount) : undefined,
    );
    assert.deepEqual(await privateBalances(asset, lastSlot), [
      0n,
      transferAmount,
    ]);

    await startServer(recipientId);
    lastSlot = await request(
      "withdraw",
      {
        mint: asset,
        recipient: recipient.address,
        amount: transferAmount.toString(),
      },
      asset === SOL_MINT ? -transferAmount : undefined,
    );
    assert.deepEqual(await privateBalances(asset, lastSlot), [0n, 0n]);

    if (asset !== SOL_MINT) {
      assert.equal(
        await publicTokens(senderAddress, asset),
        beforeSender - transferAmount,
      );
      assert.equal(
        await publicTokens(recipientAddress, asset),
        beforeRecipient + transferAmount,
      );
      const [vault] = await getProgramDerivedAddress({
        programAddress: SHIELDED_POOL_PROGRAM_ID,
        seeds: [
          new TextEncoder().encode("spl_asset_vault"),
          getAddressEncoder().encode(asset),
        ],
      });
      assert.equal(vault, await getSplAssetVaultAddress(asset));
    }
  }
  console.log("SOL and SPL confidential E2E passed");
} finally {
  try {
    await stopServer();
  } finally {
    senderKeys?.destroy();
    recipientKeys?.destroy();
  }
}
