import {
  appendTransactionMessageInstructions,
  assertIsTransactionWithBlockhashLifetime,
  createTransactionMessage,
  getSignatureFromTransaction,
  lamports,
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
import { SOL_MINT, Wallet, createSprClient, syncWallet } from "@heliuslabs/spr";
import type { SprClient } from "@heliuslabs/spr/client";
import type { ShieldedAddress } from "@heliuslabs/spr/keypair";
import {
  AssetRegistry,
  type LocalShieldedKeys,
} from "@heliuslabs/spr/transaction";

export interface Confirmed {
  readonly signature: Signature;
  readonly slot: bigint;
}

export async function localClient(): Promise<SprClient> {
  return createSprClient({
    solanaRpcUrl: process.env["SA_RPC_URL"] ?? "http://127.0.0.1:18899",
    indexerUrl: process.env["SA_INDEXER_URL"] ?? "http://127.0.0.1:18784",
    proverUrl: process.env["SA_PROVER_URL"] ?? "http://127.0.0.1:13301",
    proofDataSource: "client",
  });
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

export async function airdrop(
  client: SprClient,
  to: Address,
  amount: bigint,
): Promise<void> {
  const signature = await client.solanaRpc
    .requestAirdrop(to, lamports(amount))
    .send();
  await client.confirmTransaction(signature);
}

/**
 * Syncs a wallet for `address` until its SOL balance is `expected`; the
 * indexer lags the validator by a few slots.
 */
export async function syncedSolBalance(
  client: SprClient,
  address: ShieldedAddress,
  keys: LocalShieldedKeys,
  expected: bigint,
): Promise<Wallet> {
  let last = -1n;
  for (let attempt = 0; attempt < 60; attempt += 1) {
    const wallet = new Wallet({
      identity: address,
      registry: new AssetRegistry(),
    });
    await syncWallet({ wallet, keys, client, config: { pageLimit: 50 } });
    last = wallet.balance(SOL_MINT).amount;
    if (last === expected) return wallet;
    await new Promise((resolve) => setTimeout(resolve, 500));
  }
  throw new Error(
    `expected private SOL balance ${expected}, last synced ${last}`,
  );
}
