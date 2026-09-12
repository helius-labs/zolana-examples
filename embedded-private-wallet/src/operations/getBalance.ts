import { SOL_MINT } from "@heliuslabs/zolana";
import { PublicKey, type Connection } from "@solana/web3.js";
import type { PrivateWalletContext } from "../lib/walletContext";

/** Public SOL is independent of private-wallet activation and private sync. */
export async function getPublicSolBalance(
  connection: Pick<Connection, "getBalance">,
  owner: string
) {
  const lamports = await connection.getBalance(
    new PublicKey(owner),
    "confirmed"
  );
  if (!Number.isSafeInteger(lamports) || lamports < 0)
    throw new Error("Public balance is too large to display precisely.");
  return BigInt(lamports);
}

/** Reads the last synced private SOL balance, without a network request. */
export function getPrivateSolBalance(
  ctx: Pick<PrivateWalletContext, "wallet" | "assertActive">
) {
  ctx.assertActive();
  return ctx.wallet.balance(SOL_MINT).amount;
}
