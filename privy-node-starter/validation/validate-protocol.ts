import "dotenv/config";
import { readFile, writeFile } from "node:fs/promises";
import { homedir } from "node:os";
import { ShieldedKeypair, SigningKey } from "@heliuslabs/zolana";
import {
  address,
  createKeyPairSignerFromBytes,
  getTransactionDecoder,
  getTransactionEncoder,
  type SignatureBytes,
} from "@solana/kit";
import { getTransferSolInstruction } from "@solana-program/system";
import { clientConfig } from "../src/config.js";
import { connect, send, validateTransfers } from "../src/flow.js";
import type { RemoteWallet } from "../src/signer.js";

if (!process.env.API_KEY) throw new Error("Set API_KEY in .env");
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
const signingKeys = [SigningKey.generate(), SigningKey.generate()];
await writeFile(
  "protocol-seeds.local.json",
  JSON.stringify(signingKeys.map((key) => Array.from(key.secretBytes()))),
  { mode: 0o600, flag: "wx" },
);

function localWallet(key: SigningKey): RemoteWallet {
  const signer = ShieldedKeypair.fromKeypair(key).toSolanaSigner();
  return {
    address: signer.address,
    async signMessage() {
      return key.derivationSeed();
    },
    async signTransaction(wire) {
      const transaction = getTransactionDecoder().decode(wire);
      const signature = key.sign(new Uint8Array(transaction.messageBytes));
      return new Uint8Array(
        getTransactionEncoder().encode({
          ...transaction,
          signatures: {
            ...transaction.signatures,
            [signer.address]: new Uint8Array(signature) as SignatureBytes,
          },
        }),
      );
    },
  };
}
const sender = localWallet(signingKeys[0]!);
const recipient = localWallet(signingKeys[1]!);
await send(client, payer, [
  getTransferSolInstruction({
    source: payer,
    destination: address(sender.address),
    amount: 50_000_000n,
  }),
  getTransferSolInstruction({
    source: payer,
    destination: address(recipient.address),
    amount: 50_000_000n,
  }),
]);
console.log("Protocol validation uses local test signers; it does not validate Privy signing.");
const result = await validateTransfers(client, sender, recipient, (stage, details) =>
  console.log(stage, details ?? ""),
);
await writeFile(
  "protocol-validation.local.json",
  JSON.stringify(
    {
      provider: "local test signers; Privy unverified",
      checkedAt: new Date().toISOString(),
      ...result,
    },
    null,
    2,
  ),
);
console.log("Protocol Devnet validation passed; evidence saved to protocol-validation.local.json");
