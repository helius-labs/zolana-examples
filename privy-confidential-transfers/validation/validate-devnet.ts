import "dotenv/config";
import { readFile, writeFile } from "node:fs/promises";
import { homedir } from "node:os";
import { PrivyClient } from "@privy-io/node";
import { createKeyPairSignerFromBytes, address } from "@solana/kit";
import { getTransferSolInstruction } from "@solana-program/system";
import { clientConfig } from "../src/config.js";
import { connect, send, validateTransfers } from "../src/flow.js";
import type { RemoteWallet } from "../src/signer.js";

const appId = process.env.PRIVY_APP_ID;
const appSecret = process.env.PRIVY_APP_SECRET;
if (!appId || !appSecret)
  throw new Error("Set PRIVY_APP_ID and PRIVY_APP_SECRET in .env to validate real Privy wallets");
if (!process.env.API_KEY) throw new Error("Set API_KEY in .env");
const privy = new PrivyClient({ appId, appSecret });
const client = await connect(
  clientConfig(`https://devnet.helius-rpc.com/?api-key=${encodeURIComponent(process.env.API_KEY)}`),
);
const payerPath = (process.env.ZOLANA_PAYER_KEYPAIR ?? "~/.config/solana/id.json").replace(
  /^~(?=\/)/,
  homedir(),
);
const payer = await createKeyPairSignerFromBytes(
  Uint8Array.from(JSON.parse(await readFile(payerPath, "utf8"))),
);
const fundingAmount = 50_000_000n;
if ((await client.getBalance(payer.address)) < fundingAmount * 2n + 1_000_000n)
  throw new Error("Devnet payer needs at least 0.101 SOL");

// Reference: privy-io/examples/privy-node-starter/src/lib/privy.ts.
const created = [
  await privy.wallets().create({ chain_type: "solana" }),
  await privy.wallets().create({ chain_type: "solana" }),
];
await writeFile(
  "wallets.local.json",
  JSON.stringify(
    created.map(({ id, address }) => ({ id, address })),
    null,
    2,
  ),
  { mode: 0o600 },
);

function remote(wallet: { id: string; address: string }): RemoteWallet {
  return {
    address: wallet.address,
    async signMessage(message) {
      const result = await privy.wallets().solana().signMessage(wallet.id, { message });
      return new Uint8Array(Buffer.from(result.signature, "base64"));
    },
    async signTransaction(transaction) {
      const result = await privy.wallets().solana().signTransaction(wallet.id, { transaction });
      return new Uint8Array(Buffer.from(result.signed_transaction, "base64"));
    },
  };
}

await send(
  client,
  payer,
  created.map((wallet) =>
    getTransferSolInstruction({
      source: payer,
      destination: address(wallet.address),
      amount: fundingAmount,
    }),
  ),
);
const result = await validateTransfers(
  client,
  remote(created[0]!),
  remote(created[1]!),
  (stage, details) => console.log(stage, details ?? ""),
);
await writeFile(
  "validation.local.json",
  JSON.stringify(
    { provider: "Privy server wallets", checkedAt: new Date().toISOString(), ...result },
    null,
    2,
  ),
);
console.log("Privy Devnet validation passed; evidence saved to validation.local.json");
