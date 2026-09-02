import {
  SOL_MINT,
  createZolanaClient,
} from "@heliuslabs/zolana";
import { atSlot } from "@heliuslabs/zolana/client";
import {
  depositInstruction,
  DepositAsset,
} from "@heliuslabs/zolana/interface";
import { randomBlinding } from "@heliuslabs/zolana/keypair";
import {
  AssetRegistry,
  decryptToBalances,
} from "@heliuslabs/zolana/transaction";

import {
  sendAndConfirmFactory,
  setup,
} from "../src/lib.js";

const DEPOSIT_AMOUNT = 1_000_000_000n;

async function main(): Promise<void> {
  const {
    sender: senderKeypair,
    clientConfig,
  } = await setup();

  // Connect to the RPC, indexer, and prover.
  const client =
    await createZolanaClient(clientConfig);

  // Initialize the sender's private wallet and local authority
  // to decrypt transactions and sync balances.
  // The Solana signer and private wallet are derived from the same Ed25519 seed.
  const senderSigner =
    senderKeypair.toSolanaSigner();
  const senderAddress =
    senderKeypair.shieldedAddress();

  // The SDK hands back instructions; the app owns signing and sending.
  const sendAndConfirm = sendAndConfirmFactory(
    client,
    senderSigner,
  );

  // Mints that are registered with Solana Rings for privacy.
  const assets = new AssetRegistry();

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
  const balances = await decryptToBalances({
    keypair: senderKeypair,
    registry: assets,
    transactions: depositResponse.transactions,
  });
  const balance = balances.balance(SOL_MINT);
  if (balance.amount !== DEPOSIT_AMOUNT) {
    throw new Error(
      `expected deposit amount ${DEPOSIT_AMOUNT}, got ${balance.amount}`,
    );
  }
  if (balance.utxos.length !== 1) {
    throw new Error(
      `expected 1 deposit utxo, got ${balance.utxos.length}`,
    );
  }

  console.log(
    `ok private_balance=${balance.amount} utxos=${balance.utxos.length}`,
  );
}

await main();
