import { address } from "@solana/kit";
import {
  SOL_MINT,
  createZolanaClient,
} from "@heliuslabs/zolana";
import {
  AssetRegistry,
  decryptToBalances,
} from "@heliuslabs/zolana/transaction";

import { connect } from "../src/lib.js";

// House mock-USDC on the zolana demo.
const HOUSE_USDC_MINT = address(
  "CjHHSnWtR17GVhFmvAtBvcrvPPDU3XovsnSf3RKEySCc",
);
const HOUSE_USDC_ASSET_ID = 6n;

async function main(): Promise<void> {
  const { wallet, clientConfig } =
    await connect();

  // Connect to the RPC, indexer, and prover.
  const client =
    await createZolanaClient(clientConfig);

  // Mints that are registered with Solana Rings for privacy.
  const assets = new AssetRegistry();
  assets.insert(
    HOUSE_USDC_ASSET_ID,
    HOUSE_USDC_MINT,
  );

  // Initialize the sender's private wallet and local authority
  // to decrypt transactions and sync balances.
  // The Solana signer and private wallet are derived from the same Ed25519 seed.
  const senderSigner = wallet.toSolanaSigner();
  const senderViewTag = wallet
    .shieldedAddress()
    .confidentialViewTag();

  // Fetch transaction outputs from the indexer.
  // The indexer returns encrypted outputs by view tag.
  const response =
    await client.getShieldedTransactionsByTags({
      tags: [senderViewTag],
    });

  // The sender decrypts the transaction outputs locally to read the private balance.
  const balances = await decryptToBalances({
    keypair: wallet,
    registry: assets,
    transactions: response.transactions,
  });
  const sol = balances.balance(SOL_MINT);
  console.log(
    `ok solana_address=${senderSigner.address} sol=${sol.amount} utxos=${sol.utxos.length}`,
  );
  const spl = balances.balance(HOUSE_USDC_MINT);
  console.log(
    `ok mint=${HOUSE_USDC_MINT} spl=${spl.amount} utxos=${spl.utxos.length}`,
  );
}

await main();
