import type { RequestHandler } from "express";
import { createPrivateWallet } from "../../lib/private-wallet.js";

/**
 * Creates a private wallet: a Solana wallet held by Turnkey in your Helius
 * project, and its private keys, which the Helius enclave derives from it
 * and holds. Fund the returned address with devnet SOL before depositing.
 *
 * @example
 * curl -X POST http://localhost:3300/wallets
 */
export const createWallet: RequestHandler = async (_req, res) => {
  const wallet = await createPrivateWallet();
  res.status(201).json({ address: wallet.address });
};
