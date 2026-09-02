import {
  assertIsTransactionWithBlockhashLifetime,
  getSignatureFromTransaction,
  sendAndConfirmTransactionFactory,
  signTransactionWithSigners,
  type Transaction,
  type TransactionPartialSigner,
} from "@solana/kit";
import { createZolanaClient } from "@heliuslabs/zolana";

type Client = Awaited<ReturnType<typeof createZolanaClient>>;

export function submitFactory(
  client: Client,
  signer: TransactionPartialSigner,
) {
  const sendAndConfirm = sendAndConfirmTransactionFactory({
    rpc: client.solanaRpc,
    rpcSubscriptions: client.solanaRpcSubscriptions,
  });
  return async function submit(transaction: Transaction) {
    const signed = await signTransactionWithSigners([signer], transaction);
    assertIsTransactionWithBlockhashLifetime(signed);
    await sendAndConfirm(signed, { commitment: "confirmed" });
    const signature = getSignatureFromTransaction(signed);
    const slot = await client.confirmTransaction(signature);
    return { signature, slot };
  };
}
