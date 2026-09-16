import {
  getTransactionDecoder,
  getTransactionEncoder,
  type Address,
  type SignatureBytes,
  type TransactionPartialSigner,
} from "@solana/kit";
import { VersionedTransaction } from "@solana/web3.js";

export type SignTransactionFn = (
  transaction: VersionedTransaction,
) => Promise<VersionedTransaction>;

export function walletAdapterSigner(input: {
  address: Address;
  signTransaction: SignTransactionFn;
}): TransactionPartialSigner {
  const { address, signTransaction } = input;
  return {
    address,
    signTransactions: async (transactions) =>
      Promise.all(
        transactions.map(async (transaction) => {
          const bytes = Uint8Array.from(
            getTransactionEncoder().encode(transaction),
          );
          const signed = await signTransaction(
            VersionedTransaction.deserialize(bytes),
          );
          const decoded = getTransactionDecoder().decode(signed.serialize());
          const signature = decoded.signatures[address];
          if (!signature) {
            throw new Error("wallet did not sign as fee payer");
          }
          return Object.freeze({ [address]: signature as SignatureBytes });
        }),
      ),
  };
}
