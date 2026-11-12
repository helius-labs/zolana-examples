import { readFile } from "node:fs/promises";

import {
  AccountRole,
  appendTransactionMessageInstructions,
  assertIsTransactionWithBlockhashLifetime,
  createKeyPairSignerFromBytes,
  createTransactionMessage,
  getSignatureFromTransaction,
  pipe,
  sendTransactionWithoutConfirmingFactory,
  setTransactionMessageConfig,
  setTransactionMessageFeePayerSigner,
  setTransactionMessageLifetimeUsingBlockhash,
  signTransactionMessageWithSigners,
  type Address,
  type Instruction,
  type Signature,
  type TransactionSigner,
} from "@solana/kit";
import {
  SHIELDED_POOL_PROGRAM_ID,
  SOL_MINT,
  USER_REGISTRY_PROGRAM_ID,
  Wallet,
  createSprClient,
  syncWallet,
} from "@heliuslabs/spr";
import { SignedTeePolicy, type SprClient } from "@heliuslabs/spr/client";
import type { ShieldedAddress } from "@heliuslabs/spr/keypair";
import {
  AssetRegistry,
  type LocalShieldedKeys,
} from "@heliuslabs/spr/transaction";

import { STORAGE_PROGRAM_ADDRESS } from "../../src/storage.js";
import {
  SMART_ACCOUNT_PROGRAM_ADDRESS,
  SYSTEM_PROGRAM_ADDRESS,
  decodeProgramConfig,
  programConfigAddress,
} from "../../src/squads.js";
import { GENESIS_HASH, networkConfig } from "./config.js";

export interface Confirmed {
  readonly signature: Signature;
  readonly slot: bigint;
}

async function setupNetwork(): Promise<
  Readonly<{ client: SprClient; payer: TransactionSigner }>
> {
  const { network, rpcUrl, payerPath } = networkConfig();
  const client = await createSprClient({
    solanaRpcUrl: rpcUrl,
    proverTee: SignedTeePolicy.forNetwork(network),
    proofDataSource: "prover",
  });
  const genesis = await client.solanaRpc.getGenesisHash().send();
  if (genesis !== GENESIS_HASH[network]) {
    throw new Error(
      `SOLANA_${network.toUpperCase()}_RPC_URL is not ${network}`,
    );
  }
  for (const program of [
    SHIELDED_POOL_PROGRAM_ID,
    USER_REGISTRY_PROGRAM_ID,
    SMART_ACCOUNT_PROGRAM_ADDRESS,
    STORAGE_PROGRAM_ADDRESS,
  ]) {
    const { value } = await client.solanaRpc
      .getAccountInfo(program, { encoding: "base64" })
      .send();
    if (!value?.executable) {
      throw new Error(`${program} is not executable on ${network}`);
    }
  }
  const config = await client.getAccount(await programConfigAddress());
  if (!config || config.owner !== SMART_ACCOUNT_PROGRAM_ADDRESS) {
    throw new Error(`the Squads ProgramConfig is missing on ${network}`);
  }
  decodeProgramConfig(config.data);
  await client.attestProver();
  await client.checkProverProvingKeys();

  const secret: unknown = JSON.parse(await readFile(payerPath, "utf8"));
  if (
    !Array.isArray(secret) ||
    secret.length !== 64 ||
    secret.some((byte) => !Number.isInteger(byte) || byte < 0 || byte > 255)
  ) {
    throw new Error(`invalid Solana keypair at ${payerPath}`);
  }
  const bytes = Uint8Array.from(secret);
  let payer;
  try {
    payer = await createKeyPairSignerFromBytes(bytes);
  } finally {
    bytes.fill(0);
    secret.fill(0);
  }
  if ((await client.getBalance(payer.address)) < 100_000_000n) {
    throw new Error(`fund the ${network} payer with at least 0.1 SOL`);
  }
  console.log(
    `${network}: RPC, deployed programs and attested prover verified`,
  );
  return { client, payer };
}

let setup: ReturnType<typeof setupNetwork> | undefined;

export function networkSetup(): ReturnType<typeof setupNetwork> {
  return (setup ??= setupNetwork());
}

/** Signs with every signer the instructions name, then waits for confirmation. */
export async function send(
  client: SprClient,
  feePayer: TransactionSigner,
  instructions: readonly Instruction[],
): Promise<Confirmed> {
  const { value: lifetime } = await client.solanaRpc
    .getLatestBlockhash()
    .send();
  const signed = await signTransactionMessageWithSigners(
    pipe(
      createTransactionMessage({ version: 1 }),
      (message) => setTransactionMessageFeePayerSigner(feePayer, message),
      (message) =>
        setTransactionMessageLifetimeUsingBlockhash(lifetime, message),
      (message) =>
        setTransactionMessageConfig(
          {
            computeUnitLimit: 1_400_000,
            loadedAccountsDataSizeLimit: 64 * 1024 * 1024,
          },
          message,
        ),
      (message) => appendTransactionMessageInstructions(instructions, message),
    ),
  );
  assertIsTransactionWithBlockhashLifetime(signed);
  await sendTransactionWithoutConfirmingFactory({ rpc: client.solanaRpc })(
    signed,
    {
      commitment: "confirmed",
    },
  );
  const signature = getSignatureFromTransaction(signed);
  const slot = await client.confirmTransaction(signature);
  return { signature, slot };
}

export function transferSol(
  from: TransactionSigner | Address,
  to: Address,
  amount: bigint,
): Instruction {
  const data = new Uint8Array(12);
  data[0] = 2;
  new DataView(data.buffer).setBigUint64(4, amount, true);
  return {
    programAddress: SYSTEM_PROGRAM_ADDRESS,
    accounts: [
      {
        address: typeof from === "string" ? from : from.address,
        role: AccountRole.WRITABLE_SIGNER,
        ...(typeof from === "string" ? {} : { signer: from }),
      },
      { address: to, role: AccountRole.WRITABLE },
    ],
    data,
  };
}

/**
 * Syncs a wallet for `address` until its SOL balance is `expected`; the
 * indexer can lag the RPC.
 */
export async function syncedSolBalance(
  client: SprClient,
  address: ShieldedAddress,
  keys: LocalShieldedKeys,
  expected: bigint,
): Promise<Wallet> {
  let last = -1n;
  for (let attempt = 0; attempt < 120; attempt += 1) {
    const wallet = new Wallet({
      identity: address,
      registry: new AssetRegistry(),
    });
    await syncWallet({ wallet, keys, client, config: { pageLimit: 50 } });
    last = wallet.balance(SOL_MINT).amount;
    if (last === expected) return wallet;
    await new Promise((resolve) => setTimeout(resolve, 1000));
  }
  throw new Error(
    `expected private SOL balance ${expected}, last synced ${last}`,
  );
}
