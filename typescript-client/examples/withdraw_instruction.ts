import { SOL_MINT } from "@zolana/sdk";
import { atSlot } from "@zolana/sdk/client";
import {
  TransactWithdrawal,
  transactInstruction,
} from "@zolana/sdk/interface";
import {
  ConfidentialTransfer,
  ProofInputUtxo,
  WithdrawalTarget,
  decryptToBalances,
} from "@zolana/sdk/transaction";

import {
  sendAndConfirmInstructions,
  setupFundedWallet,
} from "../src/lib.js";

const DEPOSIT_AMOUNT = 1_000_000_000n;
const WITHDRAW_AMOUNT = 300_000_000n;

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

  // Withdraw SOL from the sender's private balance to their public balance.
  // A withdrawal reveals sender, recipient, asset, and amount.

  // 1. Select private token accounts (UTXOs) that make up the private balance
  // for the withdrawal.
  const withdrawalUtxo =
    senderWallet.balance(SOL_MINT).utxos[0]!;
  // SPL: const withdrawalUtxo = senderWallet.balance(spl.mint).utxos[0]!;

  // 2. Prepare the selected UTXOs as inputs for the zero-knowledge proof.
  const withdrawalInput =
    ProofInputUtxo.fromKeypair(
      withdrawalUtxo,
      senderKeypair,
    );

  // 3. Build and sign the private-to-public withdrawal.
  // Signing encrypts the asset and amount of the remaining private balance and
  // produces the proof inputs for the ZK prover.
  const withdrawal = new ConfidentialTransfer(
    senderAddress,
    [withdrawalInput],
    senderSigner.address,
  );
  withdrawal.withdraw(
    SOL_MINT,
    WITHDRAW_AMOUNT,
    WithdrawalTarget.sol({
      recipient: senderSigner.address,
    }),
  );
  // SPL alternative:
  // withdrawal.withdraw(
  //   spl.mint,
  //   WITHDRAW_AMOUNT,
  //   WithdrawalTarget.spl({
  //     recipientTokenAccount: spl.recipientTokenAccount,
  //     splTokenInterface: spl.splTokenInterface,
  //   }),
  // );
  const withdrawalProofInputs = withdrawal.sign(
    senderKeypair,
    assets,
  );

  // 4. Fetch the ZK proof to prove the sender can spend the balance.
  const withdrawalData =
    await client.proveTransact(
      withdrawalProofInputs,
    );

  // 5. Build the instruction with the state Merkle tree and public Solana
  // account required for the withdrawal.
  const withdrawalIx = transactInstruction({
    payer: senderSigner,
    inputTree: client.tree,
    outputTree: client.tree,
    withdrawal: TransactWithdrawal.sol({
      recipient: senderSigner.address,
    }),
    // SPL alternative:
    // withdrawal: TransactWithdrawal.spl({
    //   mint: spl.mint,
    //   splTokenInterface: spl.splTokenInterface,
    //   recipientTokenAccount: spl.recipientTokenAccount,
    //   tokenProgram: spl.tokenProgram,
    // }),
    data: withdrawalData,
  });

  // 6. Send and confirm like any Solana transaction.
  const withdrawalSignature =
    await sendAndConfirmInstructions(
      client,
      senderSigner,
      [withdrawalIx],
    );
  const slot = await client.confirmTransaction(
    withdrawalSignature,
  );

  // 7. Fetch and decrypt the sender's remaining outputs.
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

  // 8. Report the public SOL withdrawal.
  const solanaBalance = await client.getBalance(
    senderSigner.address,
  );
  console.log(
    `ok withdrawal solana_balance=${solanaBalance} remaining_private_sol=${balances.balance(SOL_MINT).amount} tx=${withdrawalSignature}`,
  );
}

await main();
