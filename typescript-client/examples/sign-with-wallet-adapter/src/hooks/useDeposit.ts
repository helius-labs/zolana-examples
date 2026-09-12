import { address } from "@solana/kit";
import {
  buildDepositTransaction,
  buildTransferTransaction,
  buildWithdrawalTransaction,
  SOL_MINT,
  syncWallet,
} from "@heliuslabs/zolana";
import type { PrivateWalletContext } from "./usePrivateWallet";

export const DEPOSIT_AMOUNT = 10_000_000n;
export const TRANSFER_AMOUNT = 3_000_000n;
export const WITHDRAW_AMOUNT = 3_000_000n;

export class BalanceSyncError extends Error {
  constructor(readonly signature: string) {
    super(
      "Transaction confirmed, but balances could not refresh. Refresh balances before making another transaction.",
    );
    this.name = "BalanceSyncError";
  }
}

async function syncAfterTransaction(
  ctx: PrivateWalletContext,
  signature: string,
  slot: bigint,
) {
  try {
    await syncWallet({ ...ctx, config: { requireSlot: slot } });
  } catch {
    throw new BalanceSyncError(signature);
  }
}

export async function depositSol(ctx: PrivateWalletContext) {
  const { client, wallet, authority, submit } = ctx;
  const tx = await buildDepositTransaction({
    client,
    feePayer: authority.solanaPublicKey(),
    recipient: await authority.shieldedAddress(),
    amount: DEPOSIT_AMOUNT,
  });
  const { signature, slot } = await submit(tx);
  await syncAfterTransaction(ctx, signature, slot);
  return { signature, privateBalance: wallet.balance(SOL_MINT).amount };
}

export async function transferSol(
  ctx: PrivateWalletContext,
  recipient: ReturnType<typeof address>,
) {
  const { client, wallet, authority, submit } = ctx;
  const tx = await buildTransferTransaction({
    client,
    wallet,
    authority,
    feePayer: authority.solanaPublicKey(),
    recipient,
    amount: TRANSFER_AMOUNT,
  });
  const { signature, slot } = await submit(tx);
  await syncAfterTransaction(ctx, signature, slot);
  return { signature, privateBalance: wallet.balance(SOL_MINT).amount };
}

export async function withdrawSol(ctx: PrivateWalletContext) {
  const { client, wallet, authority, submit } = ctx;
  const tx = await buildWithdrawalTransaction({
    client,
    wallet,
    authority,
    feePayer: authority.solanaPublicKey(),
    recipient: authority.solanaPublicKey(),
    amount: WITHDRAW_AMOUNT,
  });
  const { signature, slot } = await submit(tx);
  await syncAfterTransaction(ctx, signature, slot);
  return { signature, privateBalance: wallet.balance(SOL_MINT).amount };
}
