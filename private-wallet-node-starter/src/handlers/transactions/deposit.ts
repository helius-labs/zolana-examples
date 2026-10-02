import {
  buildDepositTransaction,
  buildRegistrationTransaction,
} from "@heliuslabs/zolana";
import type { RequestHandler } from "express";
import { lamports } from "../../lib/amount.js";
import { openPrivateWallet, send, zolana } from "../../lib/private-wallet.js";
import { loadWallet } from "../../lib/store.js";

/**
 * Moves public SOL into the wallet's private balance. A deposit reveals the
 * sender, the recipient, the asset and the amount. The first deposit also
 * registers the wallet, so others can pay it by its Solana address.
 *
 * @example
 * curl -X POST http://localhost:3300/wallets/<address>/deposit \
 * -H "Content-Type: application/json" \
 * -d '{"lamports": "10000000"}'
 */
export const deposit: RequestHandler<{ address: string }> = async (
  req,
  res,
) => {
  const stored = await loadWallet(req.params.address);
  if (!stored) {
    res.status(404).json({ error: "no such wallet" });
    return;
  }
  const amount = lamports(req.body?.lamports);
  if (amount === undefined) {
    res.status(400).json({ error: "lamports must be a positive integer" });
    return;
  }
  const client = await zolana;
  const { shieldedAddress, signer } = await openPrivateWallet(stored);
  const registration = await buildRegistrationTransaction({
    client,
    owner: signer.address,
    address: shieldedAddress,
  });
  if (registration) await send(registration, signer);
  const transaction = await buildDepositTransaction({
    client,
    feePayer: signer.address,
    recipient: shieldedAddress,
    amount,
  });
  res.json({ signature: await send(transaction, signer) });
};
