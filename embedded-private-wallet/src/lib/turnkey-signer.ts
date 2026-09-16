import {
  address,
  getPublicKeyFromAddress,
  getTransactionDecoder,
  getTransactionEncoder,
  verifySignature,
  type Address,
  type SignatureDictionary,
  type Transaction,
  type TransactionPartialSigner,
} from "@solana/kit";

export type SignSerializedTransaction = (
  transaction: Uint8Array,
) => Promise<Uint8Array>;

function sameBytes(left: ArrayLike<number>, right: ArrayLike<number>): boolean {
  if (left.length !== right.length) return false;
  for (let index = 0; index < left.length; index += 1) {
    if (left[index] !== right[index]) return false;
  }
  return true;
}

/**
 * The embedded Turnkey wallet as a `@solana/kit` signer, so the transactions
 * the Zolana SDK builds are signed by the session that owns the wallet.
 *
 * Turnkey signs a whole serialized transaction and returns it signed. The
 * adapter accepts only the signature it asked for: the message must come back
 * byte for byte, and the signature in this signer's slot must verify against
 * the wallet's public key before it is attached.
 */
export function turnkeyTransactionSigner(
  walletAddress: string,
  signTransaction: SignSerializedTransaction,
): TransactionPartialSigner {
  const signer: Address = address(walletAddress);
  const publicKey = getPublicKeyFromAddress(signer);
  const encoder = getTransactionEncoder();
  const decoder = getTransactionDecoder();
  return {
    address: signer,
    async signTransactions(
      transactions: readonly Transaction[],
    ): Promise<readonly SignatureDictionary[]> {
      const signatures: SignatureDictionary[] = [];
      for (const transaction of transactions) {
        if (!(signer in transaction.signatures)) {
          throw new Error("SignerNotRequired");
        }
        const signed = decoder.decode(
          await signTransaction(new Uint8Array(encoder.encode(transaction))),
        );
        if (!sameBytes(signed.messageBytes, transaction.messageBytes)) {
          throw new Error("SignedTransactionMismatch");
        }
        const signature = signed.signatures[signer];
        if (!signature) throw new Error("MissingTransactionSignature");
        if (!(await verifySignature(await publicKey, signature, transaction.messageBytes))) {
          throw new Error("InvalidTransactionSignature");
        }
        signatures.push({ [signer]: signature });
      }
      return signatures;
    },
  };
}
