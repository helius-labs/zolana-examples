import "dotenv/config";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { createZolanaClient, initializePoseidon } from "@heliuslabs/zolana";
import { LocalKeys, teePolicyFromJson } from "@heliuslabs/zolana/client";
import {
  ed25519DerivationMessage,
  type Bytes32,
} from "@heliuslabs/zolana/keypair";
import { getSetComputeUnitLimitInstruction } from "@solana-program/compute-budget";
import {
  address,
  appendTransactionMessageInstructions,
  assertIsTransactionWithinSizeLimit,
  compileTransaction,
  createTransactionMessage,
  getAddressEncoder,
  getTransactionEncoder,
  pipe,
  setTransactionMessageFeePayer,
  setTransactionMessageLifetimeUsingBlockhash,
  type Address,
  type Instruction,
} from "@solana/kit";
import privyClient from "./lib/privy.js";
import config from "./lib/config.js";

export const { privy } = privyClient;
export const { SENDER_WALLET_ID, TREASURY_AUTHORIZATION_KEY } = config;
export type Client = Awaited<ReturnType<typeof createZolanaClient>>;
export type SolanaWallet = { id: string; address: string };
let clientPromise: Promise<Client> | undefined;

export function getClient(): Promise<Client> {
  clientPromise ??= (async () => {
    await initializePoseidon();
    const client = await createZolanaClient({
      solanaRpcUrl: config.HELIUS_RPC_URL,
      indexerUrl: "https://d34xe1hqb2ibg2.cloudfront.net",
      proverUrl: "https://dd73hh473h6ha.cloudfront.net",
      proofDataSource: "prover",
      proverTee: teePolicyFromJson(
        JSON.parse(readFileSync("../devnet-e-tee-policy.json", "utf8")),
      ),
    });
    assert.equal(
      await client.solanaRpc.getGenesisHash().send(),
      "EtWTRABZaYq6iMfeYKouRu166VU2xqa1wcaWoxPkrZBG",
      "Use Solana Devnet",
    );
    return client;
  })();
  return clientPromise;
}

export function readAmount(value: unknown): bigint {
  if (typeof value !== "string" || !/^[0-9]+$/.test(value)) {
    throw new Error("amount must be an integer string in base units");
  }
  const amount = BigInt(value);
  if (amount <= 0n || amount > (1n << 64n) - 1n) {
    throw new Error("amount must be a positive u64");
  }
  return amount;
}

export async function deriveKeys(client: Client, wallet: SolanaWallet) {
  const owner = address(wallet.address);
  const publicKey = new Uint8Array(
    getAddressEncoder().encode(owner),
  ) as Bytes32;
  const message = ed25519DerivationMessage(publicKey);
  const result = await privy
    .wallets()
    .solana()
    .signMessage(wallet.id, {
      message,
      authorization_context: {
        authorization_private_keys: [TREASURY_AUTHORIZATION_KEY],
      },
    });
  const seed = new Uint8Array(Buffer.from(result.signature, "base64"));
  try {
    return LocalKeys.fromDerivationSeed(
      { solanaPublicKey: owner, derivationSeed: seed },
      client.proofService,
    );
  } finally {
    seed.fill(0);
  }
}

export async function loadSender(client: Client) {
  const wallet = await privy.wallets().get(SENDER_WALLET_ID);
  return {
    owner: address(wallet.address),
    keys: await deriveKeys(client, wallet),
  };
}

export async function encodeTransaction(
  client: Client,
  owner: Address,
  instructions: readonly Instruction[],
) {
  const { value: lifetime } = await client.solanaRpc
    .getLatestBlockhash()
    .send();
  const transaction = compileTransaction(
    pipe(
      createTransactionMessage({ version: 0 }),
      (message) => setTransactionMessageFeePayer(owner, message),
      (message) =>
        setTransactionMessageLifetimeUsingBlockhash(lifetime, message),
      (message) =>
        appendTransactionMessageInstructions(
          [
            getSetComputeUnitLimitInstruction({ units: 450_000 }),
            ...instructions,
          ],
          message,
        ),
    ),
  );
  assertIsTransactionWithinSizeLimit(transaction);
  return new Uint8Array(getTransactionEncoder().encode(transaction));
}
