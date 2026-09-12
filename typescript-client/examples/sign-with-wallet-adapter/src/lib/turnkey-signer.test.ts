import {
  AccountRole,
  appendTransactionMessageInstruction,
  compileTransaction,
  createTransactionMessage,
  generateKeyPairSigner,
  getTransactionDecoder,
  getTransactionEncoder,
  partiallySignTransaction,
  pipe,
  setTransactionMessageFeePayer,
  setTransactionMessageLifetimeUsingBlockhash,
  signTransactionWithSigners,
  type Blockhash,
  type KeyPairSigner,
  type Transaction,
} from "@solana/kit";
import { describe, expect, it, vi } from "vitest";
import { turnkeyTransactionSigner } from "./turnkey-signer";

const encoder = getTransactionEncoder();
const decoder = getTransactionDecoder();

async function unsignedTransfer(payer: KeyPairSigner): Promise<Transaction> {
  const recipient = await generateKeyPairSigner();
  return compileTransaction(
    pipe(
      createTransactionMessage({ version: 0 }),
      (message) => setTransactionMessageFeePayer(payer.address, message),
      (message) =>
        setTransactionMessageLifetimeUsingBlockhash(
          { blockhash: "11111111111111111111111111111111" as Blockhash, lastValidBlockHeight: 1n },
          message,
        ),
      (message) =>
        appendTransactionMessageInstruction(
          {
            programAddress: recipient.address,
            accounts: [{ address: payer.address, role: AccountRole.WRITABLE_SIGNER }],
            data: new Uint8Array([1, 2, 3]),
          },
          message,
        ),
    ),
  );
}

/** What Turnkey does: signs the serialized transaction with the wallet's key. */
function turnkeyOf(keypair: KeyPairSigner) {
  return vi.fn(async (bytes: Uint8Array) => {
    const signed = await partiallySignTransaction([keypair.keyPair], decoder.decode(bytes));
    return new Uint8Array(encoder.encode(signed));
  });
}

describe("turnkeyTransactionSigner", () => {
  it("returns the wallet's signature over the exact message", async () => {
    const wallet = await generateKeyPairSigner();
    const transaction = await unsignedTransfer(wallet);
    const turnkey = turnkeyOf(wallet);
    const signer = turnkeyTransactionSigner(wallet.address, turnkey);

    const signed = await signTransactionWithSigners([signer], transaction);
    expect(turnkey).toHaveBeenCalledTimes(1);
    expect(signed.messageBytes).toEqual(transaction.messageBytes);
    const reference = await partiallySignTransaction([wallet.keyPair], transaction);
    expect(signed.signatures[wallet.address]).toEqual(reference.signatures[wallet.address]);
  });

  it("refuses a returned transaction whose message differs", async () => {
    const wallet = await generateKeyPairSigner();
    const transaction = await unsignedTransfer(wallet);
    const other = await unsignedTransfer(wallet);
    const turnkey = turnkeyOf(wallet);
    const signer = turnkeyTransactionSigner(wallet.address, async () =>
      turnkey(new Uint8Array(encoder.encode(other))),
    );
    await expect(signTransactionWithSigners([signer], transaction)).rejects.toThrowError(
      "SignedTransactionMismatch",
    );
  });

  it("refuses a result that did not sign for this wallet", async () => {
    const wallet = await generateKeyPairSigner();
    const transaction = await unsignedTransfer(wallet);
    const signer = turnkeyTransactionSigner(wallet.address, async (bytes) => bytes);
    await expect(signTransactionWithSigners([signer], transaction)).rejects.toThrowError(
      "MissingTransactionSignature",
    );
  });

  it("refuses a signature that is not the wallet's", async () => {
    const wallet = await generateKeyPairSigner();
    const impostor = await generateKeyPairSigner();
    const transaction = await unsignedTransfer(wallet);
    const signer = turnkeyTransactionSigner(wallet.address, async (bytes) => {
      // Signed by another key, placed in the wallet's slot.
      const forged = await partiallySignTransaction([impostor.keyPair], {
        ...decoder.decode(bytes),
        signatures: { [wallet.address]: null, [impostor.address]: null },
      });
      return new Uint8Array(
        encoder.encode({
          ...transaction,
          signatures: { [wallet.address]: forged.signatures[impostor.address] ?? null },
        }),
      );
    });
    await expect(signTransactionWithSigners([signer], transaction)).rejects.toThrowError(
      "InvalidTransactionSignature",
    );
  });

  it("refuses to sign a transaction that does not name this wallet", async () => {
    const wallet = await generateKeyPairSigner();
    const stranger = await generateKeyPairSigner();
    const transaction = await unsignedTransfer(stranger);
    const turnkey = turnkeyOf(wallet);
    const signer = turnkeyTransactionSigner(wallet.address, turnkey);
    await expect(signTransactionWithSigners([signer], transaction)).rejects.toThrowError(
      "SignerNotRequired",
    );
    expect(turnkey).not.toHaveBeenCalled();
  });
});
