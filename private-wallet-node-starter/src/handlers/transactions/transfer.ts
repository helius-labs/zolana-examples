import { WalletError, buildTransferTransaction } from "@heliuslabs/zolana";
import { address, isAddress } from "@solana/kit";
import type { RequestHandler } from "express";
import { positiveAmount } from "../../lib/amount.js";
import { openPrivateWallet, zolana } from "../../lib/private-wallet.js";
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
  const amount = positiveAmount(req.body?.lamports);
  const recipient = req.body?.recipient;
  if (
    amount === undefined ||
    typeof recipient !== "string" ||
    !isAddress(recipient)
  ) {
    res.status(400).json({ error: "invalid recipient or lamports" });
    return;
  }
  const { keys, wallet, signer, send } = await openPrivateWallet(stored);
  let transaction;
  try {
    transaction = await buildTransferTransaction({
      client: await zolana,
      wallet,
      keys,
      feePayer: signer.address,
      recipient: address(recipient),
      amount,
    });
  } catch (error) {
    if (
      error instanceof WalletError &&
      [error.code, ...(error.causeCodes ?? [])].includes(
        "WALLET_RECIPIENT_NOT_REGISTERED",
      )
    ) {
      res.status(400).json({
        error:
          "the recipient has no private wallet yet; it gets one on its first deposit",
      });
      return;
    }
    throw error;
  }
  res.json({ signature: await send(transaction) });
};
