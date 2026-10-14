import { address } from "@solana/kit";
import type { RequestHandler } from "express";
import { openPrivateWallet, zolana } from "../../lib/private-wallet.js";
import { loadWallet } from "../../lib/store.js";

/**
 * The wallet's public and private SOL balances, in lamports. Reading the
 * private balance decrypts the wallet's notes in the enclave.
 *
 * @example
 * curl http://localhost:3300/wallets/<address>
 */
export const getWallet: RequestHandler<{ address: string }> = async (
  req,
  res,
) => {
  const stored = await loadWallet(req.params.address);
  if (!stored) {
    res.status(404).json({ error: "no such wallet" });
    return;
  }
  const client = await zolana;
  const opened = await openPrivateWallet(stored);
  const publicLamports = await client.getBalance(address(stored.address));
  res.json({
    address: stored.address,
    publicLamports: String(publicLamports),
    privateLamports: String(opened.privateLamports()),
  });
};
