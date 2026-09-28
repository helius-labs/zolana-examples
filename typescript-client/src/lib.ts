import "dotenv/config";

import { readFile } from "node:fs/promises";
import { homedir } from "node:os";

import {
  appendTransactionMessageInstructions,
  assertIsTransactionWithBlockhashLifetime,
  createTransactionMessage,
  getSignatureFromTransaction,
  generateKeyPairSigner,
  pipe,
  sendTransactionWithoutConfirmingFactory,
  setTransactionMessageConfig,
  setTransactionMessageFeePayerSigner,
  setTransactionMessageLifetimeUsingBlockhash,
  signTransactionMessageWithSigners,
  signTransactionWithSigners,
  type Address,
  type Instruction,
  type Signature,
  type Transaction,
  type TransactionModifyingSigner,
  type TransactionPartialSigner,
  type TransactionSigner,
} from "@solana/kit";
import {
  SigningKey,
  createZolanaClient,
  type Bytes32,
} from "@heliuslabs/zolana";
import { getCreateAccountInstruction } from "@solana-program/system";
import {
  getInitializeAccount3Instruction,
  getInitializeMint2Instruction,
  getMintSize,
  getMintToInstruction,
  getTokenSize,
  TOKEN_PROGRAM_ADDRESS,
} from "@solana-program/token";
export type Client = Awaited<ReturnType<typeof createZolanaClient>>;

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

/**
 * The Solana CLI wallet (`ZOLANA_PAYER_KEYPAIR`, defaults to
 * `~/.config/solana/id.json`).
 */
export async function cliKeypair(): Promise<SigningKey> {
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
  // public key.
  const seed = Uint8Array.from(secret.slice(0, 32)) as Bytes32;
  try {
    return SigningKey.fromEd25519Bytes(seed);
  } finally {
    seed.fill(0);
  }
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
        createTransactionMessage({ version: 1 }),
        (message) => setTransactionMessageFeePayerSigner(feePayer, message),
        (message) =>
          setTransactionMessageLifetimeUsingBlockhash(lifetime, message),
        (message) =>
          setTransactionMessageConfig(
            {
              computeUnitLimit: 450_000,
              loadedAccountsDataSizeLimit: 64 * 1024 * 1024,
            },
            message,
          ),
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

/**
 * Sign and send a compiled transaction as the given fee payer, then wait
 * for the transaction to confirm.
 *
 * `buildRegistrationTransaction` returns a compiled transaction, not
 * instructions, so this path signs that transaction instead of building
 * a new message. Confirmation reuses the SDK's `confirmTransaction`.
 */
export function sendTransactionFactory(
  client: Client,
  feePayer: TransactionPartialSigner | TransactionModifyingSigner,
): (transaction: Transaction) => Promise<ConfirmedTransaction> {
  const sendTransaction = sendTransactionWithoutConfirmingFactory({
    rpc: client.solanaRpc,
  });

  return async function sendAndConfirmTransaction(
    transaction: Transaction,
  ): Promise<ConfirmedTransaction> {
    const signed = await signTransactionWithSigners([feePayer], transaction);
    assertIsTransactionWithBlockhashLifetime(signed);
    await sendTransaction(signed, { commitment: "confirmed" });
    const signature = getSignatureFromTransaction(signed);
    const slot = await client.confirmTransaction(signature);
    return { signature, slot };
  };
}

/** Create a test mint and fund the payer's token account. */
export async function setupTestToken(
  client: Client,
  payer: TransactionSigner,
  amount: bigint,
): Promise<{ mint: Address; sourceToken: Address }> {
  const sendAndConfirm = sendAndConfirmFactory(client, payer);
  const mint = await generateKeyPairSigner();
  const sourceToken = await generateKeyPairSigner();
  const mintRent = await client.solanaRpc
    .getMinimumBalanceForRentExemption(BigInt(getMintSize()))
    .send();
  const tokenRent = await client.solanaRpc
    .getMinimumBalanceForRentExemption(BigInt(getTokenSize()))
    .send();
  await sendAndConfirm([
    getCreateAccountInstruction({
      payer,
      newAccount: mint,
      lamports: mintRent,
      space: getMintSize(),
      programAddress: TOKEN_PROGRAM_ADDRESS,
    }),
    getInitializeMint2Instruction({
      mint: mint.address,
      decimals: 9,
      mintAuthority: payer.address,
      freezeAuthority: null,
    }),
    getCreateAccountInstruction({
      payer,
      newAccount: sourceToken,
      lamports: tokenRent,
      space: getTokenSize(),
      programAddress: TOKEN_PROGRAM_ADDRESS,
    }),
    getInitializeAccount3Instruction({
      account: sourceToken.address,
      mint: mint.address,
      owner: payer.address,
    }),
    getMintToInstruction({
      mint: mint.address,
      token: sourceToken.address,
      mintAuthority: payer,
      amount,
    }),
  ]);

  return { mint: mint.address, sourceToken: sourceToken.address };
}
