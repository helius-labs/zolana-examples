import assert from "node:assert/strict";

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

// Connect to Helius devnet RPC plus the Photon indexer and prover.
const client = await createZolanaClient(clientConfig);

// Initialize the sender's local keys to decrypt transactions and sync balances.
// The Solana signer and private wallet are derived from the same Ed25519 seed.
const signer = sender.toSolanaSigner();
const owner = signer.address;
const shieldedAddress = sender.shieldedAddress();
const keys = LocalKeys.fromKeypair(sender, client.proofService);

// The SDK hands back instructions; the app owns signing and sending.
const sendAndConfirm = sendAndConfirmFactory(client, signer);

// Register the wallet and enable UTXO merge in one transaction.

// 1. Build the registration instruction with the wallet's public keys.
const userRecord = await getUserRecordAddress(owner);
const registerIx = getRegisterInstruction({
  userRecord,
  owner: signer,
  data: {
    nullifierPublicKey: shieldedAddress.nullifierPublicKey,
    viewingPublicKey: shieldedAddress.viewingPublicKey.toBytes(),
  },
});

// 2. Enable merging for the same wallet.
const enableMergingIx = getSetMergingEnabledInstruction({
  userRecord,
  owner: signer,
  enabled: true,
});

// 3. Send and confirm like any Solana transaction.
const setupTx = await sendAndConfirm([registerIx, enableMergingIx]);
console.log(`register and enable merge tx=${setupTx.signature}`);

// Deposit SOL into the sender's private balance.
// A deposit from a public balance reveals sender, recipient, asset and amount.

// 1. Move public SOL into three private token accounts (UTXOs).
// The view tag is the sender's Solana public key in confidential rings.
// Used by the indexer to fetch the sender's outputs.
const recipient = {
  asset: DepositAsset.sol(),
  viewTag: shieldedAddress.confidentialViewTag(),
  recipientOwnerHash: shieldedAddress.ownerHash(),
};

// Each deposit uses fresh blinding to create a distinct note commitment.
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

// 2. Send and confirm like any Solana transaction.
const depositTx = await sendAndConfirm([depositIx]);
console.log(`deposit tx=${depositTx.signature}`);

// 3. Sync two devices to the same private wallet, gated on the deposit's slot.
// Each device decrypts the transaction outputs locally to read the private balance.
const deviceA = new Wallet({
  identity: shieldedAddress,
});
const deviceB = new Wallet({
  identity: shieldedAddress,
});
const depositSync = {
  keys,
  client,
  config: { requireSlot: depositTx.slot },
};
await Promise.all([
  syncWallet({ ...depositSync, wallet: deviceA }),
  syncWallet({ ...depositSync, wallet: deviceB }),
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

// Merge two private token accounts into one without changing the private balance.
// Merge requires the nullifier key and decrypted UTXOs.
// Merge cannot change the owner or spend the balance.

// 1. Select private token accounts (UTXOs) that make up the private balance for the merge.
const sharedInputs = [];
for (const entry of deviceA.utxos()) {
  if (entry.spent || entry.utxo.asset !== SOL_MINT) {
    continue;
  }
  sharedInputs.push(entry.outputContext.hash);
  if (sharedInputs.length === 2) {
    break;
  }
}
if (sharedInputs.length !== 2) {
  throw new Error(`expected 2 merge inputs, got ${sharedInputs.length}`);
}

// 2. Build both devices' merges from the same inputs before either is submitted.
// This creates the two-device conflict demonstrated below.
const mergeParams = {
  client,
  keys,
  feePayer: owner,
  inputs: sharedInputs,
};
const [deviceAMerge, deviceBStaleMerge] = await Promise.all([
  buildMergeTransaction({ ...mergeParams, wallet: deviceA }),
  buildMergeTransaction({ ...mergeParams, wallet: deviceB }),
]);

// 3. Send and confirm like any Solana transaction.
const deviceATx = await signAndConfirmTransaction(client, deviceAMerge, signer);
console.log(`device A merge tx=${deviceATx.signature}`);

// 4. Submit Device B's stale merge and expect rejection.
// Device A has already spent the shared input nullifiers.
await assert.rejects(
  () => signAndConfirmTransaction(client, deviceBStaleMerge, signer),
  "stale device B merge unexpectedly succeeded",
);
console.log("device B stale merge rejected");

// Recover Device B's stale wallet state and retry the merge.

// 1. Fetch the sender's outputs again, gated on Device A's merge slot,
// and read the remaining private balance.
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

// 2. Select the remaining unspent UTXOs from the refreshed wallet.
const refreshedInputs = [];
for (const entry of deviceB.utxos()) {
  if (entry.spent || entry.utxo.asset !== SOL_MINT) {
    continue;
  }
  refreshedInputs.push(entry.outputContext.hash);
}

// 3. Rebuild the merge with fresh Merkle proofs and current rootIndex values.
// Do not resend the transaction built from stale wallet state.
const retryMerge = await buildMergeTransaction({
  client,
  wallet: deviceB,
  keys,
  feePayer: owner,
  inputs: refreshedInputs,
});

// 4. Send and confirm like any Solana transaction.
const retryTx = await signAndConfirmTransaction(client, retryMerge, signer);

// 5. Fetch the sender's outputs again, gated on the retry's slot,
// and check that one UTXO holds the original 0.3 SOL balance.
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
