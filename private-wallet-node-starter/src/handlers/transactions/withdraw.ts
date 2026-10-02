import { buildWithdrawalTransaction } from "@heliuslabs/zolana";
import { address, isAddress } from "@solana/kit";
import type { RequestHandler } from "express";
import { lamports } from "../../lib/amount.js";
import { openPrivateWallet, send, zolana } from "../../lib/private-wallet.js";
import { loadWallet } from "../../lib/store.js";

/**
 * Moves SOL from the wallet's private balance to any public Solana address,
 * the wallet's own by default. A withdrawal reveals the sender, the recipient,
 * the asset and the amount.
 *
 * @example
 * curl -X POST http://localhost:3300/wallets/<address>/withdraw \
 * -H "Content-Type: application/json" \
 * -d '{"lamports": "3000000"}'
 */
export const withdraw: RequestHandler<{ address: string }> = async (
  req,
  res,
) => {
  const stored = await loadWallet(req.params.address);
  if (!stored) {
    res.status(404).json({ error: "no such wallet" });
    return;
  }
  const amount = lamports(req.body?.lamports);
  const recipient = req.body?.recipient ?? stored.address;
  if (
    amount === undefined ||
    typeof recipient !== "string" ||
    !isAddress(recipient)
  ) {
    res.status(400).json({ error: "invalid recipient or lamports" });
    return;
  }
  const { keys, wallet, signer } = await openPrivateWallet(stored);
  const transaction = await buildWithdrawalTransaction({
    client: await zolana,
    wallet,
    keys,
    feePayer: signer.address,
    recipient: address(recipient),
    amount,
  });
  res.json({ signature: await send(transaction, signer) });
};
