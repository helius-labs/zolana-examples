import {
  LocalKeys,
  SOL_MINT,
  Wallet,
  buildMergeTransaction,
  createZolanaClient,
  syncWallet,
} from "@heliuslabs/zolana";
import { getUserRecordAddress } from "@heliuslabs/zolana/addresses";
import {
  getDepositInstructionAsync,
  getRegisterInstruction,
  getSetMergingEnabledInstruction,
} from "@heliuslabs/zolana/instructions";
import { DepositAsset } from "@heliuslabs/zolana/interface";
import { randomBlinding } from "@heliuslabs/zolana/keypair";

import {
  sendAndConfirmFactory,
  setup,
  signAndConfirmTransaction,
} from "../src/lib.js";

// Three deposits of 0.1 SOL each.
const NOTE_AMOUNT = 100_000_000n;
const NOTE_COUNT = 3;
const TOTAL_AMOUNT = NOTE_AMOUNT * 3n;

const { sender, clientConfig } = await setup();
const client = await createZolanaClient(clientConfig);
const signer = sender.toSolanaSigner();
const owner = signer.address;
const shieldedAddress = sender.shieldedAddress();
const keys = LocalKeys.fromKeypair(sender, client.proofService);
const sendAndConfirm = sendAndConfirmFactory(client, signer);

const userRecord = await getUserRecordAddress(owner);
const registerIx = getRegisterInstruction({
  userRecord,
  owner: signer,
  data: {
    nullifierPublicKey: shieldedAddress.nullifierPublicKey,
    viewingPublicKey: shieldedAddress.viewingPublicKey.toBytes(),
  },
});
const enableMergingIx = getSetMergingEnabledInstruction({
  userRecord,
  owner: signer,
  enabled: true,
});
const setupTx = await sendAndConfirm([registerIx, enableMergingIx]);
console.log(`register and enable merge tx=${setupTx.signature}`);

const recipient = {
  asset: DepositAsset.sol(),
  viewTag: shieldedAddress.confidentialViewTag(),
  recipientOwnerHash: shieldedAddress.ownerHash(),
};
const deposits = [
  { ...recipient, amount: NOTE_AMOUNT, blinding: randomBlinding() },
  { ...recipient, amount: NOTE_AMOUNT, blinding: randomBlinding() },
  { ...recipient, amount: NOTE_AMOUNT, blinding: randomBlinding() },
];
const depositIx = await getDepositInstructionAsync({
  tree: client.tree,
  depositor: signer,
  deposits,
});
const depositTx = await sendAndConfirm([depositIx]);
console.log(`deposit tx=${depositTx.signature}`);

const deviceA = new Wallet({
  identity: shieldedAddress,
});
const deviceB = new Wallet({
  identity: shieldedAddress,
});
await Promise.all([
  syncWallet({
    wallet: deviceA,
    keys,
    client,
    config: { requireSlot: depositTx.slot },
  }),
  syncWallet({
    wallet: deviceB,
    keys,
    client,
    config: { requireSlot: depositTx.slot },
  }),
]);

const deviceABalance = deviceA.balance(SOL_MINT);
const deviceBBalance = deviceB.balance(SOL_MINT);
if (
  deviceABalance.amount !== TOTAL_AMOUNT ||
  deviceABalance.utxos.length !== NOTE_COUNT ||
  deviceBBalance.amount !== TOTAL_AMOUNT ||
  deviceBBalance.utxos.length !== NOTE_COUNT
) {
  throw new Error("expected both devices to hold three notes totaling 0.3 SOL");
}

const sharedInputs = deviceA
  .utxos()
  .filter((entry) => !entry.spent && entry.utxo.asset === SOL_MINT)
  .slice(0, 2)
  .map((entry) => entry.outputContext.hash);
if (sharedInputs.length !== 2) {
  throw new Error(`expected 2 merge inputs, got ${sharedInputs.length}`);
}

const [deviceAMerge, deviceBStaleMerge] = await Promise.all([
  buildMergeTransaction({
    client,
    wallet: deviceA,
    keys,
    feePayer: owner,
    inputs: sharedInputs,
  }),
  buildMergeTransaction({
    client,
    wallet: deviceB,
    keys,
    feePayer: owner,
    inputs: sharedInputs,
  }),
]);
const deviceATx = await signAndConfirmTransaction(client, deviceAMerge, signer);
console.log(`device A merge tx=${deviceATx.signature}`);

const staleSubmission = await signAndConfirmTransaction(
  client,
  deviceBStaleMerge,
  signer,
).then(
  (transaction) => ({ transaction }),
  (error) => ({ error }),
);
if ("transaction" in staleSubmission) {
  throw new Error(
    `stale device B merge unexpectedly succeeded: ${staleSubmission.transaction.signature}`,
  );
}
console.log(`device B stale merge rejected: ${String(staleSubmission.error)}`);

await syncWallet({
  wallet: deviceB,
  keys,
  client,
  config: { requireSlot: deviceATx.slot },
});
const refreshedBalance = deviceB.balance(SOL_MINT);
if (
  refreshedBalance.amount !== TOTAL_AMOUNT ||
  refreshedBalance.utxos.length !== 2
) {
  throw new Error(
    "expected the refreshed wallet to hold two notes totaling 0.3 SOL",
  );
}

const refreshedInputs = deviceB
  .utxos()
  .filter((entry) => !entry.spent && entry.utxo.asset === SOL_MINT)
  .map((entry) => entry.outputContext.hash);
const retryMerge = await buildMergeTransaction({
  client,
  wallet: deviceB,
  keys,
  feePayer: owner,
  inputs: refreshedInputs,
});
const retryTx = await signAndConfirmTransaction(client, retryMerge, signer);

await syncWallet({
  wallet: deviceB,
  keys,
  client,
  config: { requireSlot: retryTx.slot },
});
const finalBalance = deviceB.balance(SOL_MINT);
if (finalBalance.amount !== TOTAL_AMOUNT || finalBalance.utxos.length !== 1) {
  throw new Error("expected the retry to consolidate 0.3 SOL into one note");
}
console.log(`device B retry merge tx=${retryTx.signature}`);
