import "dotenv/config";

import { readFile } from "node:fs/promises";
import { homedir } from "node:os";

import {
  appendTransactionMessageInstructions,
  assertIsTransactionWithBlockhashLifetime,
  createTransactionMessage,
  getSignatureFromTransaction,
  pipe,
  sendTransactionWithoutConfirmingFactory,
  setTransactionMessageFeePayerSigner,
  setTransactionMessageLifetimeUsingBlockhash,
  signTransactionMessageWithSigners,
  type Instruction,
  type Signature,
  type TransactionSigner,
} from "@solana/kit";
import { ShieldedKeypair, createZolanaClient, type Bytes32 } from "@zolana/sdk";

export type Client = Awaited<ReturnType<typeof createZolanaClient>>;
export type Signer = ReturnType<ShieldedKeypair["toSolanaSigner"]>;

export interface ExampleContext {
  readonly client: Client;
  readonly signer: Signer;
  readonly keypair: ShieldedKeypair;
}

export interface ConfirmedTransaction {
  readonly signature: Signature;
  /** Slot the transaction landed in; drives the indexer freshness gates. */
  readonly slot: bigint;
}

function expandedPath(value: string): string {
  return value === "~"
    ? homedir()
    : value.startsWith("~/")
      ? `${homedir()}/${value.slice(2)}`
      : value;
}

async function senderKeys(): Promise<
  Readonly<{ signer: Signer; keypair: ShieldedKeypair }>
> {
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
    const keypair = ShieldedKeypair.fromEd25519(seed, 0);
    return Object.freeze({
      signer: keypair.toSolanaSigner(),
      keypair,
    });
  } finally {
    seed.fill(0);
  }
}

export async function exampleContext(): Promise<ExampleContext> {
  const endpoint = process.env["ZOLANA_ENDPOINT"]?.trim();
  const indexerUrl = process.env["ZOLANA_INDEXER_URL"]?.trim();
  const proverUrl = process.env["ZOLANA_PROVER_URL"]?.trim();
  // Client construction initializes Poseidon before shielded keys are derived.
  // With no endpoint the SDK uses its local validator, Photon, and prover URLs.
  const client = await createZolanaClient(
    endpoint || indexerUrl || proverUrl
      ? {
          ...(endpoint ? { solanaRpcUrl: endpoint } : {}),
          ...(indexerUrl ? { indexerUrl } : {}),
          ...(proverUrl ? { proverUrl } : {}),
        }
      : {},
  );
  const { signer, keypair } = await senderKeys();
  return Object.freeze({ client, signer, keypair });
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
