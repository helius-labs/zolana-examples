import {
  SOL_MINT,
  ShieldedKeypair,
} from "@zolana/sdk";
import { atSlot } from "@zolana/sdk/client";
import { transactInstruction } from "@zolana/sdk/interface";
import {
  ConfidentialTransfer,
  ProofInputUtxo,
  decryptToBalances,
} from "@zolana/sdk/transaction";

import {
  sendAndConfirmInstructions,
  setupFundedWallet,
} from "../src/lib.js";

const DEPOSIT_AMOUNT = 1_000_000_000n;
const TRANSFER_AMOUNT = 300_000_000n;

async function main(): Promise<void> {
  // Setup: connect and fund the sender's private SOL balance.
  const {
    client,
    signer: senderSigner,
    keypair: senderKeypair,
    wallet: senderWallet,
    assets,
  } = await setupFundedWallet(DEPOSIT_AMOUNT);
  const senderAddress =
    senderKeypair.shieldedAddress();
  const recipient =
    ShieldedKeypair.generate().shieldedAddress();

  // Confidential SOL transfer to the recipient's private balance.
  // A confidential transfer reveals only sender and recipient, not the asset
  // or amount.

  // 1. Select private token accounts (UTXOs) that make up the private balance
  // for the transfer.
  const transferUtxo =
    senderWallet.balance(SOL_MINT).utxos[0]!;
  // SPL: const transferUtxo = senderWallet.balance(spl.mint).utxos[0]!;

  // 2. Prepare the selected UTXOs as inputs for the zero-knowledge proof.
  const transferInput =
    ProofInputUtxo.fromKeypair(
      transferUtxo,
      senderKeypair,
    );

  // 3. Build and sign the confidential transfer.
  // Signing encrypts the asset and amount and produces the proof inputs for the
  // ZK prover.
  const transfer = new ConfidentialTransfer(
    senderAddress,
    [transferInput],
    senderSigner.address,
  );
  transfer.send(
    recipient,
    SOL_MINT,
    TRANSFER_AMOUNT,
  );
  // SPL: transfer.send(
  // SPL:   recipient,
  // SPL:   spl.mint,
  // SPL:   TRANSFER_AMOUNT,
  // SPL: );
  const transferProofInputs = transfer.sign(
    senderKeypair,
    assets,
  );

  // 4. Fetch the ZK proof to prove the sender can spend the balance without
  // revealing asset and amount.
  const transferData = await client.proveTransact(
    transferProofInputs,
  );

  // 5. Build the instruction with the state Merkle tree and Solana accounts
  // required for the transfer. Private transfers move balances only between
  // private token accounts, not public token accounts.
  const transferIx = transactInstruction({
    payer: senderSigner,
    inputTree: client.tree,
    outputTree: client.tree,
    data: transferData,
  });

  // 6. Send and confirm like any Solana transaction.
  const transferSignature =
    await sendAndConfirmInstructions(
      client,
      senderSigner,
      [transferIx],
    );
  const slot = await client.confirmTransaction(
    transferSignature,
  );

  // 7. Fetch the sender's outputs again and read the remaining private balance.
  const response =
    await client.getShieldedTransactionsByTags(
      {
        tags: [
          senderAddress.confidentialViewTag(),
        ],
      },
      atSlot(slot),
    );
  const balances = await decryptToBalances({
    keypair: senderKeypair,
    registry: assets,
    transactions: response.transactions,
  });

  console.log(
    `ok private transfer signature=${transferSignature} remaining_private_sol=${balances.balance(SOL_MINT).amount}`,
  );
}

await main();
