import { SOL_MINT } from "@heliuslabs/zolana";
import { assertLamports } from "../lib/parseSol";
import type { PrivateWalletContext } from "../lib/walletContext";
import { syncAfterTransaction } from "../lib/syncAfterTransaction";
import { buildTransferTransaction } from "@heliuslabs/zolana";
import { TRANSFER_AMOUNT } from "../lib/amounts";
import type { address } from "@solana/kit";
import type { WalletKeys } from "@heliuslabs/zolana";
import type { TransferProgressCallback } from "../lib/transferProgress";

export async function transferSol(
  ctx: PrivateWalletContext,
  recipient: ReturnType<typeof address>,
  amount = TRANSFER_AMOUNT,
  onProgress?: TransferProgressCallback
) {
  const { client, wallet, keys, owner, submit } = ctx;
  ctx.assertActive();
  assertLamports(amount);
  const report: TransferProgressCallback = (stage, signature) => {
    ctx.assertActive();
    onProgress?.(stage, signature);
  };
  report("preparing");
  // Keep TVC methods bound to their original instance; never copy key material.
  const progressKeys: WalletKeys = {
    address: () => keys.address(),
    viewingPublicKeys: () => keys.viewingPublicKeys(),
    decrypt: (...args) => keys.decrypt(...args),
    derive: (...args) => keys.derive(...args),
    transactionKeys: (...args) => keys.transactionKeys(...args),
    proveMerge: (...args) => keys.proveMerge(...args),
    prove: (...args) => {
      report("proving");
      return keys.prove(...args);
    },
  };
  const tx = await buildTransferTransaction(
    {
      client,
      wallet,
      keys: progressKeys,
      feePayer: owner,
      recipient,
      amount,
    },
    { signal: ctx.signal }
  );
  ctx.assertActive();
  report("signing");
  const { signature, slot } = await submit(tx, () => report("sending"));
  report("confirmed", signature);
  await syncAfterTransaction(ctx, signature, slot);
  report("done", signature);
  return { signature, privateBalance: wallet.balance(SOL_MINT).amount };
}
