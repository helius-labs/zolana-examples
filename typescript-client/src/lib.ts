import "dotenv/config";

import { readFile } from "node:fs/promises";
import { homedir } from "node:os";

import {
  address,
  appendTransactionMessageInstructions,
  assertIsFullySignedTransaction,
  assertIsTransactionWithBlockhashLifetime,
  createTransactionMessage,
  getSignatureFromTransaction,
  pipe,
  sendAndConfirmTransactionFactory,
  setTransactionMessageFeePayerSigner,
  setTransactionMessageLifetimeUsingBlockhash,
  signTransactionMessageWithSigners,
  signTransactionWithSigners,
  type Address,
  type Instruction,
  type Signature,
  type Transaction,
} from "@solana/kit";
import {
  LocalWalletAuthority,
  ShieldedKeypair,
  Wallet,
  buildDepositTransaction,
  buildRegistrationTransaction,
  createZolanaClient,
  syncWallet,
  walletAuthorityFromSync,
  type Bytes32,
} from "@zolana/sdk";
import { AssetRegistry } from "@zolana/sdk/transaction";

export const DEFAULT_RECIPIENT = "DNRJcGsGR6SGEYuNAaRtbZ8a86snwVcH5CJh1VcSLxx";

export type Client = Awaited<ReturnType<typeof createZolanaClient>>;
export type Signer = ReturnType<ShieldedKeypair["toSolanaSigner"]>;

export interface ExampleContext {
  readonly client: Client;
  readonly signer: Signer;
  readonly keypair: ShieldedKeypair;
}

export interface FundedWallet extends ExampleContext {
  readonly assets: AssetRegistry;
  readonly authority: LocalWalletAuthority;
  readonly wallet: Wallet;
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
  // Client construction initializes Poseidon before shielded keys are derived.
  // With no endpoint the SDK uses its local validator, Photon, and prover URLs.
  const client = await createZolanaClient(
    endpoint ? { solanaRpcUrl: endpoint } : {},
  );
  const { signer, keypair } = await senderKeys();
  return Object.freeze({ client, signer, keypair });
}

export function recipientAddress(): Address {
  return address(process.env["ZOLANA_RECIPIENT"] ?? DEFAULT_RECIPIENT);
}

function transactionSender(client: Client) {
  return sendAndConfirmTransactionFactory({
    rpc: client.solanaRpc,
    rpcSubscriptions: client.solanaRpcSubscriptions,
  });
}

export async function sendAndConfirmTransaction(
  client: Client,
  signer: Signer,
  transaction: Transaction,
): Promise<Signature> {
  const signed = await signTransactionWithSigners([signer], transaction);
  assertIsFullySignedTransaction(signed);
  assertIsTransactionWithBlockhashLifetime(signed);
  await transactionSender(client)(signed, {
    commitment: "confirmed",
  });
  return getSignatureFromTransaction(signed);
}

export async function sendAndConfirmInstructions(
  client: Client,
  signer: Signer,
  instructions: readonly Instruction[],
): Promise<Signature> {
  const { value: lifetime } = await client.solanaRpc
    .getLatestBlockhash()
    .send();
  const signed = await signTransactionMessageWithSigners(
    pipe(
      createTransactionMessage({ version: 0 }),
      (message) => setTransactionMessageFeePayerSigner(signer, message),
      (message) =>
        setTransactionMessageLifetimeUsingBlockhash(lifetime, message),
      (message) => appendTransactionMessageInstructions(instructions, message),
    ),
  );
  assertIsTransactionWithBlockhashLifetime(signed);
  await transactionSender(client)(signed, {
    commitment: "confirmed",
  });
  return getSignatureFromTransaction(signed);
}

/**
 * Setup shorthand used by transfer, withdrawal, and balance examples. The
 * examples themselves keep the operation under discussion visible.
 */
export async function setupFundedWallet(amount: bigint): Promise<FundedWallet> {
  const context = await exampleContext();
  const { client, signer, keypair } = context;
  const registration = await buildRegistrationTransaction({
    client,
    owner: signer.address,
    address: keypair.shieldedAddress(),
  });
  if (registration) {
    await sendAndConfirmTransaction(client, signer, registration);
  }

  const assets = new AssetRegistry();
  const wallet = new Wallet({
    identity: keypair.shieldedAddress(),
    registry: assets,
  });
  const authority = new LocalWalletAuthority({
    solanaPublicKey: signer.address,
    keypair,
  });

  const deposit = await buildDepositTransaction({
    client,
    feePayer: signer.address,
    recipient: keypair.shieldedAddress(),
    amount,
  });
  const depositSignature = await sendAndConfirmTransaction(
    client,
    signer,
    deposit,
  );
  await client.confirmPrivateTransaction(depositSignature);
  await syncWallet({
    client,
    wallet,
    authority,
    config: { waitForIndexer: true },
  });

  return Object.freeze({
    ...context,
    assets,
    authority,
    wallet,
  });
}

export { walletAuthorityFromSync };
