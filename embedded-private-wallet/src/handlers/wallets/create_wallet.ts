import type { RequestHandler } from "express";
import { createPrivateWallet } from "../../lib/private-wallet.js";

/** Creates a Turnkey wallet and enrolls its private-wallet keys. */
export const createWallet: RequestHandler = async (_req, res) => {
  const wallet = await createPrivateWallet();
  res.status(201).json({ address: wallet.address });
};
