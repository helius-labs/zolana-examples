import {
  SOL_MINT,
  ShieldedKeypair,
} from "@zolana/sdk";
import { atSlot } from "@zolana/sdk/client";
import {
  depositInstruction,
  transactInstruction,
  DepositAsset,
} from "@zolana/sdk/interface";
import { randomBlinding } from "@zolana/sdk/keypair";
import {
  AssetRegistry,
  ConfidentialTransfer,
  ProofInputUtxo,
  decryptToBalances,
} from "@zolana/sdk/transaction";

import {
  exampleContext,
  sendAndConfirmFactory,
} from "../src/lib.js";

const DEPOSIT_AMOUNT = 1_000_000_000n;
const TRANSFER_AMOUNT = 300_000_000n;

async function main(): Promise<void> {
  const {
    client,
    signer: senderSigner,
    keypair: senderKeypair,
  } = await exampleContext();
  const recipient =
    ShieldedKeypair.generate().shieldedAddress();
  const sendAndConfirm = sendAndConfirmFactory(
    client,
    senderSigner,
  );
  const assets = new AssetRegistry();

  // Initialize the sender's private wallet and local authority
  // to decrypt transactions and sync balances.
  // The Solana signer and private wallet are derived from the same Ed25519 seed.
  const senderAddress =
    senderKeypair.shieldedAddress();

  // Deposit SOL into the sender's private balance.
  // A deposit from a public balance reveals
  // sender, recipient, asset and amount.
  // Alternatively, you can onramp fiat directly to a private balance.

  // 1. Move public SOL into the sender's private balance.
  // The view tag is the sender's Solana public key in confidential rings.
  // Used by the indexer to fetch the sender's outputs.
  const senderViewTag =
    senderAddress.confidentialViewTag();
  const depositIx = await depositInstruction({
    tree: client.tree,
    depositor: senderSigner,
    deposits: [
      {
        asset: DepositAsset.sol(),
        viewTag: senderViewTag,
        recipientOwnerHash:
          senderAddress.ownerHash(),
        blinding: randomBlinding(),
        amount: DEPOSIT_AMOUNT,
      },
    ],
  });

  // 2. Send and confirm like any Solana transaction; confirmation yields the landed slot.
  const depositTx = await sendAndConfirm([
    depositIx,
  ]);

  // 3. Fetch transaction outputs from the indexer, gated on the deposit's slot.
  // The indexer returns encrypted outputs by view tag.
  const depositResponse =
    await client.getShieldedTransactionsByTags(
      { tags: [senderViewTag] },
      atSlot(depositTx.slot),
    );

  // 4. The sender decrypts the transaction outputs locally to read the private balance.
  const balancesAfterDeposit =
    await decryptToBalances({
      keypair: senderKeypair,
      registry: assets,
      transactions: depositResponse.transactions,
    });
  const depositBalance =
    balancesAfterDeposit.balance(SOL_MINT);
  if (depositBalance.amount !== DEPOSIT_AMOUNT) {
    throw new Error(
      `expected deposit amount ${DEPOSIT_AMOUNT}, got ${depositBalance.amount}`,
    );
  }
  if (depositBalance.utxos.length !== 1) {
    throw new Error(
      `expected 1 deposit utxo, got ${depositBalance.utxos.length}`,
    );
  }

  // Confidential SOL transfer to the recipient's private balance.
  // A confidential transfer reveals only sender and recipient,
  // not the asset or amount.

  // 1. Select private token accounts (UTXOs) that make up the private balance for the transfer.
  const transferUtxo = depositBalance.utxos[0]!;

  // 2. Prepare the selected UTXOs as inputs for the zero-knowledge proof.
  const transferInput =
    ProofInputUtxo.fromKeypair(
      transferUtxo,
      senderKeypair,
    );

  // 3. Build and sign the confidential transfer.
  // Signing encrypts the asset and amount and produces the proof inputs for the ZK prover.
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
  const transferProofInputs = transfer.sign(
    senderKeypair,
    assets,
  );

  // 4. Fetch the ZK proof to prove the sender can spend the balance without revealing asset and amount.
  const transferData = await client.proveTransact(
    transferProofInputs,
  );

  // 5. Build the instruction with the state Merkle tree and Solana accounts required for the transfer.
  // Private transfers move balances only between private token accounts, not public token accounts.
  const transferInstruction = transactInstruction(
    {
      payer: senderSigner,
      inputTree: client.tree,
      outputTree: client.tree,
      data: transferData,
    },
  );

  // 6. Send and confirm like any Solana transaction; confirmation yields the landed slot.
  const transferTx = await sendAndConfirm([
    transferInstruction,
  ]);

  // 7. Fetch the sender's outputs again, gated on the transfer's slot,
  // and read the remaining private balance.
  const transferResponse =
    await client.getShieldedTransactionsByTags(
      { tags: [senderViewTag] },
      atSlot(transferTx.slot),
    );
  const balancesAfterTransfer =
    await decryptToBalances({
      keypair: senderKeypair,
      registry: assets,
      transactions: transferResponse.transactions,
    });
  const transferBalance =
    balancesAfterTransfer.balance(SOL_MINT);
  if (
    transferBalance.amount !==
    DEPOSIT_AMOUNT - TRANSFER_AMOUNT
  ) {
    throw new Error(
      `expected remaining amount ${DEPOSIT_AMOUNT - TRANSFER_AMOUNT}, got ${transferBalance.amount}`,
    );
  }
  if (transferBalance.utxos.length !== 1) {
    throw new Error(
      `expected 1 transfer utxo, got ${transferBalance.utxos.length}`,
    );
  }

  console.log(
    `ok private transfer remaining_private_sol=${transferBalance.amount} tx=${transferTx.signature}`,
  );
}

await main();
