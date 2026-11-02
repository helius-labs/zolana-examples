import assert from "node:assert/strict";
import { test } from "node:test";
import { initializePoseidon } from "@heliuslabs/zolana";
import type { ProofService } from "@heliuslabs/zolana/client";
import {
  blockhash,
  compileTransaction,
  createSignableMessage,
  createTransactionMessage,
  generateKeyPairSigner,
  getTransactionDecoder,
  getTransactionEncoder,
  setTransactionMessageFeePayer,
  setTransactionMessageLifetimeUsingBlockhash,
  type Address,
  type SignatureBytes,
} from "@solana/kit";
import { activate, equalBytes, remoteSigner, type RemoteWallet } from "./signer.js";

const proofs: ProofService = {
  async prove() {
    throw new Error("No proof requested in signer tests");
  },
  async proveMerge() {
    throw new Error("No proof requested in signer tests");
  },
};

async function fixture(): Promise<RemoteWallet> {
  const signer = await generateKeyPairSigner();
  return {
    address: signer.address,
    async signMessage(message) {
      const [signed] = await signer.signMessages([createSignableMessage(message)]);
      return new Uint8Array(signed![signer.address]!);
    },
    async signTransaction(wire) {
      const transaction = getTransactionDecoder().decode(wire);
      const signature = new Uint8Array(
        await crypto.subtle.sign(
          "Ed25519",
          signer.keyPair.privateKey,
          new Uint8Array(transaction.messageBytes),
        ),
      );
      return new Uint8Array(
        getTransactionEncoder().encode({
          ...transaction,
          signatures: { ...transaction.signatures, [signer.address]: signature as SignatureBytes },
        }),
      );
    },
  };
}

test("canonical message signatures recover the same private wallet", async () => {
  await initializePoseidon();
  const wallet = await fixture();
  const first = await activate(wallet, proofs);
  const restored = await activate(wallet, proofs);
  try {
    assert.equal(first.address().solanaAddress(), wallet.address);
    assert.ok(equalBytes(first.address().toBytes(), restored.address().toBytes()));
  } finally {
    first.destroy();
    restored.destroy();
  }
});

test("activation rejects a signature from a different Solana wallet", async () => {
  const wallet = await fixture();
  const other = await fixture();
  await assert.rejects(activate({ ...wallet, signMessage: other.signMessage }, proofs));
});

test("remote signer returns a real signature for the original Kit message", async () => {
  const wallet = await fixture();
  const signer = remoteSigner(wallet);
  const transaction = unsigned(signer.address);
  const [signatures] = await signer.signTransactions([transaction]);
  assert.equal(signatures![signer.address]!.length, 64);
  assert.ok(signatures![signer.address]!.some((byte) => byte !== 0));
});

test("remote signer rejects an unsigned response", async () => {
  const wallet = await fixture();
  const signer = remoteSigner({
    ...wallet,
    async signTransaction(wire) {
      return wire;
    },
  });
  const transaction = unsigned(signer.address);
  await assert.rejects(signer.signTransactions([transaction]), /did not sign/);
});

function unsigned(owner: Address) {
  return compileTransaction(
    setTransactionMessageLifetimeUsingBlockhash(
      { blockhash: blockhash("11111111111111111111111111111111"), lastValidBlockHeight: 1n },
      setTransactionMessageFeePayer(owner, createTransactionMessage({ version: 0 })),
    ),
  );
}
