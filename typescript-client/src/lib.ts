import "dotenv/config";

import { readFile } from "node:fs/promises";
import { homedir } from "node:os";

import {
  AccountRole,
  address,
  appendTransactionMessageInstructions,
  assertIsTransactionWithBlockhashLifetime,
  createSolanaRpc,
  createSolanaRpcSubscriptions,
  createTransactionMessage,
  getSignatureFromTransaction,
  pipe,
  sendAndConfirmTransactionFactory,
  sendTransactionWithoutConfirmingFactory,
  setTransactionMessageFeePayerSigner,
  setTransactionMessageLifetimeUsingBlockhash,
  signTransactionMessageWithSigners,
  type Address,
  type Instruction,
  type Signature,
  type TransactionSigner,
} from "@solana/kit";
import {
  ShieldedKeypair,
  createZolanaClient,
  initializePoseidon,
  type Bytes32,
  type ZolanaClientConfig,
} from "@heliuslabs/zolana";

export type Client = Awaited<ReturnType<typeof createZolanaClient>>;

export interface ExampleSetup {
  readonly sender: ShieldedKeypair;
  readonly recipient: ShieldedKeypair;
  readonly clientConfig: ZolanaClientConfig;
}

export interface ConfirmedTransaction {
  readonly signature: Signature;
  /** Slot the transaction landed in; drives the indexer freshness gates. */
  readonly slot: bigint;
}

// Will be exposed through a single devnet URL. Currently exposed as they are.
const RPC_URL = "https://devnet.helius-rpc.com";
const INDEXER_URL =
  "http://zolnet-devnet-1779374825.eu-north-1.elb.amazonaws.com";
const PROVER_URL =
  "http://zolnet-devnet-1779374825.eu-north-1.elb.amazonaws.com:3001";
// localnet: const RPC_URL = "http://127.0.0.1:8899";
// localnet: const INDEXER_URL = "http://127.0.0.1:8784";
// localnet: const PROVER_URL = "http://127.0.0.1:3001";
const SYSTEM_PROGRAM = address("11111111111111111111111111111111");
const SENDER_LAMPORTS = 2_000_000_000n;

function expandedPath(value: string): string {
  return value === "~"
    ? homedir()
    : value.startsWith("~/")
      ? `${homedir()}/${value.slice(2)}`
      : value;
}

async function funderKeypair(): Promise<ShieldedKeypair> {
  const payerPath = expandedPath(
    process.env["ZOLANA_PAYER_KEYPAIR"] ?? "~/.config/solana/id.json",
  );
  const secret = JSON.parse(await readFile(payerPath, "utf8")) as unknown;
  if (
    !Array.isArray(secret) ||
    secret.length < 32 ||
    secret.some((byte) => !Number.isInteger(byte) || byte < 0 || byte > 255)
  ) {
    throw new Error(`invalid Solana keypair at ${payerPath}`);
  }

  // Solana CLI keypair files contain the 32-byte Ed25519 seed followed by the
  // public key. Derive the Solana signer and private wallet from the same seed.
  const seed = Uint8Array.from(secret.slice(0, 32)) as Bytes32;
  try {
    return ShieldedKeypair.fromEd25519(seed, 0);
  } finally {
    seed.fill(0);
  }
}

function clientConfigFromEnv(): ZolanaClientConfig {
  const endpoint = process.env["ZOLANA_ENDPOINT"]?.trim();
  const apiKey = process.env["API_KEY"]?.trim();
  const solanaRpcUrl =
    endpoint || (apiKey ? `${RPC_URL}/?api-key=${apiKey}` : undefined);
  // localnet: const solanaRpcUrl = endpoint || RPC_URL;
  if (!solanaRpcUrl) {
    throw new Error("set API_KEY or ZOLANA_ENDPOINT");
  }
  return Object.freeze({
    solanaRpcUrl,
    indexerUrl: process.env["ZOLANA_INDEXER_URL"]?.trim() || INDEXER_URL,
    proverUrl: process.env["ZOLANA_PROVER_URL"]?.trim() || PROVER_URL,
    // The Photon/prover ALB is HTTP. Loopback HTTP is already allowed.
    allowInsecureHttp: true,
  });
}

function subscriptionsUrl(rpcUrl: string): string {
  const url = new URL(rpcUrl);
  url.protocol = url.protocol === "https:" ? "wss:" : "ws:";
  return url.href;
}

function transferSolIx(
  from: Address,
  to: Address,
  lamports: bigint,
): Instruction {
  const data = new Uint8Array(12);
  const view = new DataView(data.buffer);
  view.setUint32(0, 2, true);
  view.setBigUint64(4, lamports, true);
  return {
    programAddress: SYSTEM_PROGRAM,
    accounts: [
      { address: from, role: AccountRole.WRITABLE_SIGNER },
      { address: to, role: AccountRole.WRITABLE },
    ],
    data,
  };
}

async function fundSender(
  rpcUrl: string,
  funder: TransactionSigner,
  recipient: Address,
): Promise<void> {
  const rpc = createSolanaRpc(rpcUrl);
  const sendAndConfirm = sendAndConfirmTransactionFactory({
    rpc,
    rpcSubscriptions: createSolanaRpcSubscriptions(subscriptionsUrl(rpcUrl)),
  });
  const { value: lifetime } = await rpc.getLatestBlockhash().send();
  const signed = await signTransactionMessageWithSigners(
    pipe(
      createTransactionMessage({ version: 0 }),
      (message) => setTransactionMessageFeePayerSigner(funder, message),
      (message) =>
        setTransactionMessageLifetimeUsingBlockhash(lifetime, message),
      (message) =>
        appendTransactionMessageInstructions(
          [transferSolIx(funder.address, recipient, SENDER_LAMPORTS)],
          message,
        ),
    ),
  );
  assertIsTransactionWithBlockhashLifetime(signed);
  await sendAndConfirm(signed, { commitment: "confirmed" });
}

export async function setup(): Promise<ExampleSetup> {
  await initializePoseidon();
  const clientConfig = clientConfigFromEnv();
  const funder = await funderKeypair();
  const sender = ShieldedKeypair.generate();
  await fundSender(
    String(clientConfig.solanaRpcUrl),
    funder.toSolanaSigner(),
    sender.toSolanaSigner().address,
  );
  return Object.freeze({
    sender,
    recipient: ShieldedKeypair.generate(),
    clientConfig,
  });
}

/**
 * Sign and send instructions as the given fee payer, then wait for the
 * transaction to confirm.
 *
 * The SDK returns instructions and leaves signing and sending to the
 * application, so a Kit app owns this step. It lives here rather than in the
 * example so the example stays about the shielded-pool calls. The SDK's
 * `confirmTransaction` is the confirmation, and the status response that
 * confirms also carries the landed slot, so no request is issued twice.
 */
export function sendAndConfirmFactory(
  client: Client,
  feePayer: TransactionSigner,
): (instructions: readonly Instruction[]) => Promise<ConfirmedTransaction> {
  const sendTransaction = sendTransactionWithoutConfirmingFactory({
    rpc: client.solanaRpc,
  });

  return async function sendAndConfirm(
    instructions: readonly Instruction[],
  ): Promise<ConfirmedTransaction> {
    const { value: lifetime } = await client.solanaRpc
      .getLatestBlockhash()
      .send();
    const signed = await signTransactionMessageWithSigners(
      pipe(
        createTransactionMessage({ version: 0 }),
        (message) => setTransactionMessageFeePayerSigner(feePayer, message),
        (message) =>
          setTransactionMessageLifetimeUsingBlockhash(lifetime, message),
        (message) =>
          appendTransactionMessageInstructions(instructions, message),
      ),
    );
    assertIsTransactionWithBlockhashLifetime(signed);
    await sendTransaction(signed, { commitment: "confirmed" });
    const signature = getSignatureFromTransaction(signed);
    const slot = await client.confirmTransaction(signature);
    return { signature, slot };
  };
}
