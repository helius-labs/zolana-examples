import type { Address } from "@solana/kit";
import { sha256Bytes } from "@heliuslabs/zolana/keypair";
import { SOL_MINT, type ShieldedAddress } from "@heliuslabs/zolana";
import {
  ProofInputUtxo,
  SPP_SUPPORTED_SHAPES,
  encryptConfidentialTransfer,
  type AssetRegistry,
  type PreparedTransfer,
  type WalletUtxo,
} from "@heliuslabs/zolana/transaction";
import type { PrivateWalletContext } from "./walletContext";

const equal = (a: Uint8Array, b: Uint8Array) =>
  a.length === b.length && a.every((byte, i) => byte === b[i]);
const maxInputs = Math.max(
  ...SPP_SUPPORTED_SHAPES.map((shape) => shape.inputs),
);

/** Same plain-note, single-tree, largest-first policy as the SDK wallet builder. */
export function selectSolInputs(
  ctx: PrivateWalletContext,
  notes: readonly WalletUtxo[],
  amount: bigint,
) {
  const eligible = notes.filter(
    (e) =>
      !e.spent &&
      e.utxo.asset === SOL_MINT &&
      e.utxo.ringProgramId === undefined &&
      e.ringDataHash === undefined &&
      e.dataHash === undefined &&
      e.utxo.data.isEmpty(),
  );
  const trees = new Set(eligible.map((e) => e.outputContext.tree));
  if (trees.size > 1)
    throw new Error(
      "Spendable SOL spans multiple trees; this example requires a single tree.",
    );
  if (trees.size && !trees.has(ctx.client.tree))
    throw new Error("Spendable notes belong to a different tree.");
  const sorted = [...eligible].sort((a, b) =>
    a.utxo.amount > b.utxo.amount ? -1 : a.utxo.amount < b.utxo.amount ? 1 : 0,
  );
  const total = sorted.reduce((sum, e) => sum + e.utxo.amount, 0n);
  if (total > 0xffffffffffffffffn)
    throw new Error("Selected SOL balance exceeds the supported amount.");
  if (total < amount)
    throw new Error("Amount exceeds your spendable private SOL balance.");
  let selected = 0n;
  const entries: WalletUtxo[] = [];
  for (const entry of sorted.slice(0, maxInputs)) {
    entries.push(entry);
    selected += entry.utxo.amount;
    if (selected >= amount) break;
  }
  if (selected < amount)
    throw new Error(
      "This amount requires too many notes. Use a smaller amount or consolidate them first.",
    );
  const identity = ctx.keys.address();
  return entries.map(
    (entry) =>
      new ProofInputUtxo({
        utxo: entry.utxo,
        nullifierPublicKey: identity.nullifierPublicKey,
        nullifier: entry.nullifier,
      }),
  );
}

type SpendIntent = {
  amount: bigint;
  recipient?: ShieldedAddress;
  withdrawalRecipient?: Address;
};
/** Only SDK cryptography: prepare -> TVC transaction key -> encrypt -> finalize -> prove. */
export async function proveSpend(
  ctx: PrivateWalletContext,
  prepared: PreparedTransfer,
  registry: AssetRegistry,
  intent: SpendIntent,
  onProving?: () => void,
) {
  ctx.assertActive();
  const identity = ctx.keys.address();
  if (
    identity.solanaAddress() !== ctx.owner ||
    prepared.payer !== ctx.owner ||
    !equal(prepared.owner.toBytes(), identity.toBytes())
  )
    throw new Error("Private spend belongs to another wallet.");
  const outputs = prepared.outputs.filter((o) => !o.isDummy());
  const change = prepared.outputs
    .slice(0, prepared.senderOutputCount)
    .filter((o) => !o.isDummy());
  const recipients = prepared.outputs
    .slice(prepared.senderOutputCount)
    .filter((o) => !o.isDummy());
  const mismatch = () => {
    throw new Error(
      "Prepared transaction does not match the requested amount or recipient.",
    );
  };
  if (
    outputs.some(
      (o) => o.asset !== SOL_MINT || o.ringProgramId !== undefined,
    ) ||
    change.some(
      (o) =>
        !o.ownerAddress || !equal(o.ownerAddress.toBytes(), identity.toBytes()),
    )
  )
    mismatch();
  const inputTotal = prepared.inputs.reduce(
    (sum, i) => sum + i.utxo.amount,
    0n,
  );
  if (
    change.reduce((sum, o) => sum + o.amount, 0n) !==
    inputTotal - intent.amount
  )
    mismatch();
  if (intent.recipient) {
    if (
      prepared.interfaceTransfers.length ||
      recipients.some(
        (o) =>
          !o.ownerAddress ||
          !equal(o.ownerAddress.toBytes(), intent.recipient!.toBytes()),
      ) ||
      recipients.reduce((sum, o) => sum + o.amount, 0n) !== intent.amount
    )
      mismatch();
  } else {
    const settlement = prepared.interfaceTransfers[0];
    if (
      recipients.length ||
      prepared.interfaceTransfers.length !== 1 ||
      settlement?.kind !== "sol" ||
      settlement.isDeposit ||
      settlement.amount !== intent.amount ||
      settlement.userSolAccount !== (intent.withdrawalRecipient ?? ctx.owner)
    )
      mismatch();
  }
  const request = { signal: ctx.signal };
  const transactionKeys = await ctx.keys.transactionKeys(
    [
      {
        viewingPublicKey: identity.viewingPublicKey,
        firstNullifier: prepared.firstNullifier,
      },
    ],
    request,
  );
  let proofInputs;
  try {
    ctx.assertActive();
    if (transactionKeys.length !== 1)
      throw new Error("TVC returned an unexpected number of transaction keys.");
    proofInputs = prepared.finalize(
      encryptConfidentialTransfer(transactionKeys[0], {
        outputs: prepared.outputs,
        assets: registry,
      }),
    );
  } finally {
    transactionKeys.forEach((key) => key.destroy());
  }
  ctx.assertActive();
  // Bind methods to the TVC instance; announce proving only when the prover is called.
  const proofAuthority = {
    prove: (...args: Parameters<typeof ctx.keys.prove>) => {
      ctx.assertActive();
      onProving?.();
      return ctx.keys.prove(...args);
    },
    proveMerge: ctx.keys.proveMerge.bind(ctx.keys),
  };
  const data = await ctx.client.proveTransact(
    proofInputs,
    proofAuthority,
    undefined,
    request,
  );
  ctx.assertActive();
  if (!equal(sha256Bytes(data.privateTxHash), proofInputs.messageHash()))
    mismatch();
  const settlement = data.interfaceTransfers[0];
  if (
    intent.recipient
      ? data.interfaceTransfers.length > 0
      : data.interfaceTransfers.length !== 1 ||
        settlement?.kind !== "solWithdrawal" ||
        settlement.amount !== intent.amount
  )
    mismatch();
  return data;
}
