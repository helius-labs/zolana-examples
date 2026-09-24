import { vi } from "vitest";
import {
  blockhash,
  getAddressEncoder,
  getBase58Decoder,
  signature,
  type Address,
} from "@solana/kit";
import { LocalKeys, SOL_MINT } from "@heliuslabs/zolana";
import {
  ShieldedKeypair,
  SigningKey,
  type Bytes32,
} from "@heliuslabs/zolana/keypair";
import { DEFAULT_TREE_ADDRESS } from "@heliuslabs/zolana/interface";
import {
  AssetRegistry,
  ConfidentialTransfer,
  ProofInputUtxo,
  Utxo,
  encryptConfidentialTransfer,
  privateTxHash,
  type IndexedShieldedTransaction,
  type WalletUtxo,
  type SppProofInputs,
} from "@heliuslabs/zolana/transaction";
import type { PrivateWalletContext } from "../walletContext";

export const bytes = (n: number) => new Uint8Array(32).fill(n) as Bytes32;
export const keypair = (n: number) =>
  ShieldedKeypair.fromKeypair(SigningKey.fromEd25519Bytes(bytes(n)));
export const sig = (n: number) =>
  signature(getBase58Decoder().decode(new Uint8Array(64).fill(n)));
export const indexContext = { slot: 100n, blockTime: 1n };

// Public SDK decoders verify this fixed Borsh proofless-deposit fixture.
export function depositEvent(
  key: ShieldedKeypair,
  amount: bigint,
  leaf = 1,
  mint: Address = SOL_MINT,
) {
  const identity = key.shieldedAddress();
  const blinding = bytes(leaf);
  const utxo = new Utxo({
    owner: identity.signingPublicKey,
    asset: mint,
    amount,
    blinding,
  });
  const body = new Uint8Array(110);
  body.set(identity.ownerHash(), 0);
  body.set(blinding, 32);
  body.set(getAddressEncoder().encode(mint), 64);
  new DataView(body.buffer).setBigUint64(96, amount, true);
  const payload = new Uint8Array(6 + body.length);
  new DataView(payload.buffer).setUint32(1, body.length + 1, true);
  payload.set(body, 6);
  const outputContext = {
    tree: DEFAULT_TREE_ADDRESS,
    leafIndex: BigInt(leaf),
    hash: utxo.hash(identity.nullifierPublicKey),
  };
  const note: WalletUtxo = {
    utxo,
    outputContext,
    nullifier: key.nullifier(outputContext.hash, blinding),
    spent: false,
  };
  const event: IndexedShieldedTransaction = {
    txSignature: sig(leaf),
    slot: BigInt(leaf),
    outputSlots: [
      { outputContext, viewTag: identity.confidentialViewTag(), payload },
    ],
    messages: [],
    nullifiers: [],
    proofless: true,
  };
  return {
    event,
    note,
    match: {
      slot: event.slot,
      txSignature: event.txSignature,
      outputSlot: event.outputSlots[0],
    },
  };
}

/** Encrypt an actual SDK private transfer from a previously discovered note. */
export function spendEvent(
  key: ShieldedKeypair,
  note: WalletUtxo,
  recipient: ShieldedKeypair,
  amount: bigint,
  leaf = 20,
) {
  const identity = key.shieldedAddress();
  const input = new ProofInputUtxo({
    utxo: note.utxo,
    nullifierPublicKey: identity.nullifierPublicKey,
    nullifier: note.nullifier,
  });
  const transfer = new ConfidentialTransfer(
    identity,
    [input],
    identity.solanaAddress(),
  );
  transfer.send(recipient.shieldedAddress(), SOL_MINT, amount);
  const prepared = transfer.prepare();
  const txKey = key.transactionViewingKey(prepared.firstNullifier);
  try {
    const encrypted = encryptConfidentialTransfer(txKey, {
      outputs: prepared.outputs,
      assets: new AssetRegistry(),
    });
    const proof = prepared.finalize(encrypted);
    const event: IndexedShieldedTransaction = {
      txSignature: sig(leaf),
      slot: BigInt(leaf),
      proofless: false,
      txViewingPublicKey: encrypted.txViewingPublicKey,
      salt: encrypted.salt,
      outputSlots: proof.externalData.outputs.map((output, i) => ({
        viewTag: proof.externalData.resolvedOwnerTags[i],
        payload: output.data!,
        outputContext: {
          tree: DEFAULT_TREE_ADDRESS,
          leafIndex: BigInt(leaf + i),
          hash: output.utxoHash,
        },
      })),
      messages: proof.externalData.messages,
      nullifiers: proof.inputUtxos.map((i) => i.nullifier()),
    };
    return event;
  } finally {
    txKey.destroy();
  }
}

export function fixtureContext(key = keypair(7)) {
  const controller = new AbortController();
  const keys = LocalKeys.fromKeypair(key, {
    prove: vi.fn(),
    proveMerge: vi.fn(),
  });
  const client = {
    tree: DEFAULT_TREE_ADDRESS,
    getShieldedTransactionsByTags: vi.fn(async () => ({
      context: indexContext,
      transactions: [],
    })),
    getEncryptedUtxosByTags: vi.fn(async () => ({
      context: indexContext,
      matches: [],
    })),
    getShieldedTransactionsByNullifiers: vi.fn(async () => ({
      context: indexContext,
      transactions: [],
    })),
    getLatestBlockhash: vi.fn(async () => ({
      blockhash: blockhash(SOL_MINT),
      lastValidBlockHeight: 100n,
    })),
    getBalance: vi.fn(async () => 10_000_000n),
  };
  const ctx = {
    owner: keys.address().solanaAddress(),
    keys,
    client,
    signal: controller.signal,
    assertActive: () => controller.signal.throwIfAborted(),
    submit: vi.fn(async () => ({ signature: sig(99), slot: 99n })),
  } as unknown as PrivateWalletContext;
  return { ctx, client, controller, key, keys };
}

/** Prover response shape built from real SDK proof inputs; no network proof. */
export function instructionData(input: SppProofInputs) {
  const ext = input.externalData;
  return {
    expiryUnixTs: ext.expiryUnixTs,
    privateTxHash: privateTxHash({
      inputHashes: input.inputUtxos.map((i) =>
        i.isDummy() ? bytes(0) : i.hash(),
      ),
      outputHashes: input.outputs.map((o) =>
        o.isDummy() ? bytes(0) : o.hash(),
      ),
      externalDataHash: ext.hash(),
    }),
    circuit: {
      kind: "confidentialEddsa" as const,
      inputs: input.inputUtxos.length,
      outputs: input.outputs.length,
      publicAssetSlots: 3,
    },
    txViewingPk: ext.txViewingPublicKey.toBytes(),
    salt: ext.salt,
    proof: { a: bytes(0), b: new Uint8Array(64), c: bytes(0) },
    inputs: input.inputUtxos.map((i) => ({
      nullifierHash: i.nullifier(),
      nullifierTreeRootIndex: 0,
      utxoTreeRootIndex: 0,
    })),
    interfaceTransfers: ext.interfaceTransfers.map((t) => ({
      kind: "solWithdrawal" as const,
      amount: t.amount,
    })),
    outputs: ext.outputs,
    messages: ext.messages,
  };
}
