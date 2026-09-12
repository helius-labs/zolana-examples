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
  assertActive: () => void = () => {}
) {
  const sendAndConfirm = sendAndConfirmTransactionFactory({
    rpc: client.solanaRpc,
    rpcSubscriptions: client.solanaRpcSubscriptions,
  });
  return async function submit(
    transaction: Transaction,
    onSending?: () => void
  ) {
    assertActive();
    const signed = await signTransactionWithSigners([signer], transaction);
    assertActive();
    assertIsTransactionWithBlockhashLifetime(signed);
    onSending?.();
    await sendAndConfirm(signed, { commitment: "confirmed" });
    const signature = getSignatureFromTransaction(signed);
    const slot = await client.confirmTransaction(signature);
    return { signature, slot };
  };
}
