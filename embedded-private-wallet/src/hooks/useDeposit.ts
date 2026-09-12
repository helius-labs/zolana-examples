import { address } from "@solana/kit";
import {
  buildDepositTransaction,
  buildTransferTransaction,
  buildWithdrawalTransaction,
  SOL_MINT,
  syncWallet,
  type WalletKeys,
} from "@heliuslabs/zolana";
import type { TransferProgressCallback } from "../lib/transferProgress";
import { assertLamports } from "../lib/parseSol";
import type { PrivateWalletContext } from "./usePrivateWallet";

export const DEPOSIT_AMOUNT = 10_000_000n;
export const TRANSFER_AMOUNT = 3_000_000n;
export const WITHDRAW_AMOUNT = 3_000_000n;

export class BalanceSyncError extends Error {
  constructor(readonly signature: string) {
    super(
      "Transaction confirmed, but balances could not refresh. Refresh balances before making another transaction."
    );
    this.name = "BalanceSyncError";
  }
}

async function syncAfterTransaction(
  ctx: PrivateWalletContext,
  signature: string,
  slot: bigint
) {
  try {
    ctx.assertActive();
    await syncWallet(
      { ...ctx, config: { requireSlot: slot } },
      { signal: ctx.signal }
    );
    ctx.assertActive();
  } catch {
    throw new BalanceSyncError(signature);
  }
}

export async function depositSol(
  ctx: PrivateWalletContext,
  amount = DEPOSIT_AMOUNT
) {
  const { client, wallet, keys, owner, submit } = ctx;
  ctx.assertActive();
  assertLamports(amount);
  const tx = await buildDepositTransaction(
    {
      client,
      feePayer: owner,
      recipient: keys.address(),
      amount,
    },
    { signal: ctx.signal }
  );
  ctx.assertActive();
  const { signature, slot } = await submit(tx);
  await syncAfterTransaction(ctx, signature, slot);
  return { signature, privateBalance: wallet.balance(SOL_MINT).amount };
}

export async function transferSol(
  ctx: PrivateWalletContext,
  recipient: ReturnType<typeof address>,
  amount = TRANSFER_AMOUNT,
  onProgress?: TransferProgressCallback
) {
  const { client, wallet, keys, owner, submit } = ctx;
  ctx.assertActive();
  assertLamports(amount);
  const report: TransferProgressCallback = (stage, signature) => {
    ctx.assertActive();
    onProgress?.(stage, signature);
  };
  report("preparing");
  // Keep TVC methods bound to their original instance; never copy key material.
  const progressKeys: WalletKeys = {
    address: () => keys.address(),
    viewingPublicKeys: () => keys.viewingPublicKeys(),
    decrypt: (...args) => keys.decrypt(...args),
    derive: (...args) => keys.derive(...args),
    transactionKeys: (...args) => keys.transactionKeys(...args),
    proveMerge: (...args) => keys.proveMerge(...args),
    prove: (...args) => {
      report("proving");
      return keys.prove(...args);
    },
  };
  const tx = await buildTransferTransaction(
    {
      client,
      wallet,
      keys: progressKeys,
      feePayer: owner,
      recipient,
      amount,
    },
    { signal: ctx.signal }
  );
  ctx.assertActive();
  report("signing");
  const { signature, slot } = await submit(tx, () => report("sending"));
  report("confirmed", signature);
  await syncAfterTransaction(ctx, signature, slot);
  report("done", signature);
  return { signature, privateBalance: wallet.balance(SOL_MINT).amount };
}

export async function withdrawSol(
  ctx: PrivateWalletContext,
  amount = WITHDRAW_AMOUNT
) {
  const { client, wallet, keys, owner, submit } = ctx;
  ctx.assertActive();
  assertLamports(amount);
  const tx = await buildWithdrawalTransaction(
    {
      client,
      wallet,
      keys,
      feePayer: owner,
      recipient: owner,
      amount,
    },
    { signal: ctx.signal }
  );
  ctx.assertActive();
  const { signature, slot } = await submit(tx);
  await syncAfterTransaction(ctx, signature, slot);
  return { signature, privateBalance: wallet.balance(SOL_MINT).amount };
}
