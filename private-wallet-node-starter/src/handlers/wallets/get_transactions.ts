import type { RequestHandler } from "express";
import { openPrivateWallet } from "../../lib/private-wallet.js";
import { loadWallet } from "../../lib/store.js";

/**
 * The wallet's private transactions, newest first: deposits, private
 * transfers in and out, and withdrawals, decrypted by the enclave.
 *
 * @example
 * curl http://localhost:3300/wallets/<address>/transactions
 */
export const getTransactions: RequestHandler<{ address: string }> = async (
  req,
  res,
) => {
  const stored = await loadWallet(req.params.address);
  if (!stored) {
    res.status(404).json({ error: "no such wallet" });
    return;
  }
  const { wallet } = await openPrivateWallet(stored);
  const transactions = wallet
    .privateTransactions()
    .toSorted((a, b) => Number(b.id.slot - a.id.slot))
    .map((tx) => ({
      signature: tx.id.signature,
      slot: String(tx.id.slot),
      kind: tx.kind,
      direction: tx.direction,
      asset: tx.asset,
      amount: String(tx.amount),
    }));
  res.json({ transactions });
};
