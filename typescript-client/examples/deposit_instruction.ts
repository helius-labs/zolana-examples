import {
  DEFAULT_TREE_ADDRESS,
  LocalWalletAuthority,
  SOL_MINT,
  Wallet,
} from "@zolana/sdk";
import {
  DepositAsset,
  depositInstruction,
} from "@zolana/sdk/interface";
import { randomBlinding } from "@zolana/sdk/keypair";
import {
  AssetRegistry,
  decryptTransactions,
} from "@zolana/sdk/transaction";

import { exampleContext } from "../src/lib.js";

const DEPOSIT_AMOUNT = 1_000_000_000n;

async function main(): Promise<void> {
  // Load the funded fee payer and network settings, then connect.
  const {
    client,
    signer: senderSigner,
    keypair: senderKeypair,
  } = await exampleContext();

  // Mints that are interoperable with Solana Rings are registered with the
  // AssetRegistry. SOL is built in, so its registry starts empty.
  const assets = new AssetRegistry();

  // Initialize the sender's private wallet and local authority to decrypt
  // transactions and sync balances. The Solana signer and private wallet are
  // derived from the same Ed25519 seed.
  const senderAddress =
    senderKeypair.shieldedAddress();
  const senderWallet = new Wallet({
    identity: senderAddress,
    registry: assets,
  });
  const senderAuthority =
    new LocalWalletAuthority({
      solanaPublicKey: senderSigner.address,
      keypair: senderKeypair,
    });

  // Deposit SOL into the sender's private balance.
  // A deposit from a public balance reveals sender, recipient, asset, and
  // amount. Alternatively, you can onramp fiat directly to a private balance.

  // 1. Move public SOL into the sender's private balance.
  const senderViewTag =
    senderAddress.viewingPublicKey.x();
  const depositIx = await depositInstruction({
    tree: DEFAULT_TREE_ADDRESS,
    sender: senderSigner,
    deposits: [
      {
        asset: DepositAsset.sol(),
        // SPL alternative:
        // asset: DepositAsset.spl({
        //   mint: spl.mint,
        //   sourceTokenAccount: spl.sourceTokenAccount,
        //   tokenProgram: spl.tokenProgram,
        // }),
        viewTag: senderViewTag,
        recipientOwnerHash:
          senderAddress.ownerHash(),
        blinding: randomBlinding(),
        amount: DEPOSIT_AMOUNT,
      },
    ],
  });

  // 2. Send like any Solana transaction.
  const depositSignature =
    await client.signAndSendInstructions({
      feePayer: senderSigner,
      instructions: [depositIx],
    });
  await client.confirmPrivateTransaction(
    depositSignature,
    [senderViewTag],
  );

  // 3. Fetch transaction outputs from the indexer. The indexer returns
  // encrypted outputs by view tag: the sender's public viewing key in
  // Confidential Rings.
  const depositResponse =
    await client.getShieldedTransactionsByTags(
      senderViewTag,
    );

  // 4. The sender decrypts the transaction outputs locally to update the
  // private balance.
  await decryptTransactions({
    wallet: senderWallet,
    authority: senderAuthority,
    transactions: depositResponse.transactions,
  });

  console.log(
    `ok deposit signature=${depositSignature} private_sol=${senderWallet.balance(SOL_MINT).amount}`,
  );
}

await main();
