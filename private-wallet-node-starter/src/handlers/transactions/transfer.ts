import { buildTransferTransaction } from "@heliuslabs/zolana";
import { address, isAddress } from "@solana/kit";
import type { RequestHandler } from "express";
import { lamports } from "../../lib/amount.js";
import { openPrivateWallet, send, zolana } from "../../lib/private-wallet.js";
import { loadWallet } from "../../lib/store.js";

/**
 * Sends SOL from the wallet's private balance to another private wallet, by
 * its Solana address. A private transfer reveals the sender and the recipient,
 * not the asset or the amount. The recipient must have registered, as every
 * wallet does on its first deposit.
 *
 * @example
 * curl -X POST http://localhost:3300/wallets/<address>/transfer \
 * -H "Content-Type: application/json" \
 * -d '{"recipient": "<address>", "lamports": "3000000"}'
 */
export const transfer: RequestHandler<{ address: string }> = async (
  req,
  res,
) => {
  const stored = await loadWallet(req.params.address);
  if (!stored) {
    res.status(404).json({ error: "no such wallet" });
    return;
  }
  const amount = lamports(req.body?.lamports);
  const recipient = req.body?.recipient;
  if (
    amount === undefined ||
    typeof recipient !== "string" ||
    !isAddress(recipient)
  ) {
    res.status(400).json({ error: "invalid recipient or lamports" });
    return;
  }
  const { keys, wallet, signer } = await openPrivateWallet(stored);
  const transaction = await buildTransferTransaction({
    client: await zolana,
    wallet,
    keys,
    feePayer: signer.address,
    recipient: address(recipient),
    amount,
  });
  res.json({ signature: await send(transaction, signer) });
};
