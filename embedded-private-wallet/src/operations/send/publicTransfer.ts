import {
  address,
  createNoopSigner,
  getBase64Decoder,
  type Address,
  type TransactionMessageBytesBase64,
} from "@solana/kit";
import { getTransferSolInstruction } from "@solana-program/system";
import { assertLamports } from "../../lib/parseSol";
import { compileInstructions } from "../../lib/compileInstructions";
import { formatSol } from "../../lib/formatSol";
import type { PublicWalletContext } from "../../lib/walletContext";
import type { TransferProgressCallback } from "../../lib/transferProgress";

/** A standard SOL transfer. No registry, private reads, or TVC operations. */
export async function transferPublicSol(
  ctx: PublicWalletContext,
  recipient: Address,
  amount: bigint,
  onProgress?: TransferProgressCallback,
) {
  ctx.assertActive();
  assertLamports(amount);
  const destination = address(recipient);
  onProgress?.("preparing");
  const instruction = getTransferSolInstruction({
    // This supplies account metadata only. ctx.submit uses the verified Turnkey signer.
    source: createNoopSigner(ctx.owner),
    destination,
    amount,
  });
  const transaction = await compileInstructions(ctx, [instruction]);
  const [balance, fee] = await Promise.all([
    ctx.client.getBalance(ctx.owner, { signal: ctx.signal }),
    ctx.client.solanaRpc
      .getFeeForMessage(
        getBase64Decoder().decode(
          transaction.messageBytes,
        ) as TransactionMessageBytesBase64,
        { commitment: "confirmed" },
      )
      .send({ abortSignal: ctx.signal }),
  ]);
  ctx.assertActive();
  if (fee.value === null)
    throw new Error("Couldn’t estimate the network fee. Try again.");
  if (balance < amount + fee.value)
    throw new Error(
      `Not enough public SOL. Leave ${formatSol(fee.value)} SOL for the network fee.`,
    );
  onProgress?.("signing");
  const result = await ctx.submit(transaction, () => onProgress?.("sending"));
  ctx.assertActive();
  onProgress?.("confirmed", result.signature);
  return result;
}
