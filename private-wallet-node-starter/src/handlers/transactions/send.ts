import { address, isAddress } from "@solana/kit";
import type { RequestHandler } from "express";
import { positiveAmount } from "../../lib/amount.js";
import { openPrivateWallet } from "../../lib/private-wallet.js";
import { publicSolTransfer, publicSplTransfer } from "../../lib/solana.js";
import { loadWallet } from "../../lib/store.js";

/**
 * A public transfer from the wallet: SOL with `lamports`, or an SPL token
 * with `mint` and `amount` in the token's base units. Public transfers reveal
 * everything, as on any Solana wallet.
 *
 * @example
 * curl -X POST http://localhost:3300/wallets/<address>/send \
 * -H "Content-Type: application/json" \
 * -d '{"recipient": "<address>", "lamports": "1000000"}'
 *
 * curl -X POST http://localhost:3300/wallets/<address>/send \
 * -H "Content-Type: application/json" \
 * -d '{"recipient": "<address>", "mint": "<mint>", "amount": "250000"}'
 */
export const send: RequestHandler<{ address: string }> = async (req, res) => {
  const stored = await loadWallet(req.params.address);
  if (!stored) {
    res.status(404).json({ error: "no such wallet" });
    return;
  }
  const { recipient, mint } = req.body ?? {};
  if (typeof recipient !== "string" || !isAddress(recipient)) {
    res.status(400).json({ error: "invalid recipient" });
    return;
  }
  const opened = await openPrivateWallet(stored);
  if (mint === undefined) {
    const lamports = positiveAmount(req.body?.lamports);
    if (lamports === undefined) {
      res.status(400).json({ error: "lamports must be a positive integer" });
      return;
    }
    const transaction = await publicSolTransfer(
      opened.signer,
      address(recipient),
      lamports,
    );
    res.json({ signature: await opened.send(transaction) });
    return;
  }
  const amount = positiveAmount(req.body?.amount);
  if (typeof mint !== "string" || !isAddress(mint) || amount === undefined) {
    res.status(400).json({ error: "invalid mint or amount" });
    return;
  }
  const transaction = await publicSplTransfer(
    opened.signer,
    address(recipient),
    address(mint),
    amount,
  );
  res.json({ signature: await opened.send(transaction) });
};
