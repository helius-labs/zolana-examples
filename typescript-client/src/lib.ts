import "dotenv/config";

import { readFile } from "node:fs/promises";
import { homedir } from "node:os";

import {
  address,
  createKeyPairSignerFromPrivateKeyBytes,
  type Address,
} from "@solana/kit";
import {
  LocalWalletAuthority,
  ShieldedKeypair,
  Wallet,
  createZolanaClient,
  deposit,
  ensureRegistered,
  syncWallet,
  type Bytes32,
} from "@zolana/sdk";
import { AssetRegistry } from "@zolana/sdk/transaction";

export const DEFAULT_RECIPIENT = "DNRJcGsGR6SGEYuNAaRtbZ8a86snwVcH5CJh1VcSLxx";

export type Client = Awaited<ReturnType<typeof createZolanaClient>>;
export type Signer = Awaited<
  ReturnType<typeof createKeyPairSignerFromPrivateKeyBytes>
>;

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
  const signer = await createKeyPairSignerFromPrivateKeyBytes(seed);
  const keypair = ShieldedKeypair.fromEd25519(seed, 0);
  seed.fill(0);
  return Object.freeze({ signer, keypair });
}

export async function exampleContext(): Promise<ExampleContext> {
  const endpoint = process.env["ZOLANA_ENDPOINT"]?.trim();
  // Client construction initializes Poseidon before shielded keys are derived.
  // With no endpoint the SDK uses its local validator, Photon, and prover URLs.
  const client = await createZolanaClient(
    endpoint === "" ? undefined : endpoint,
  );
  const { signer, keypair } = await senderKeys();
  return Object.freeze({ client, signer, keypair });
}

export function recipientAddress(): Address {
  return address(process.env["ZOLANA_RECIPIENT"] ?? DEFAULT_RECIPIENT);
}

/**
 * Setup shorthand used by transfer, withdrawal, and balance examples. The
 * examples themselves keep the operation under discussion visible.
 */
export async function setupFundedWallet(amount: bigint): Promise<FundedWallet> {
  const context = await exampleContext();
  const { client, signer, keypair } = context;
  await ensureRegistered({
    client,
    funding: signer,
    keypair,
  });

  const assets = new AssetRegistry();
  const wallet = new Wallet({
    identity: keypair.shieldedAddress(),
    registry: assets,
  });
  const authority = new LocalWalletAuthority({
    solanaPublicKey: signer.address,
    keypair,
  });

  await deposit({
    client,
    sender: signer,
    recipient: keypair.shieldedAddress(),
    amount,
  });
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
