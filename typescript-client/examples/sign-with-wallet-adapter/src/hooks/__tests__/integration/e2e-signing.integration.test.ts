import { readFileSync } from "node:fs";
import { homedir } from "node:os";
import { resolve } from "node:path";
import { config } from "dotenv";
import { describe, expect, it } from "vitest";
import {
  Connection,
  Keypair,
  SystemProgram,
  Transaction,
} from "@solana/web3.js";
import { ed25519 } from "@noble/curves/ed25519.js";
import { address, getAddressEncoder } from "@solana/kit";
import {
  buildRegistrationTransaction,
  syncWallet,
  Wallet,
} from "@heliuslabs/zolana";
import { isWalletRegistered } from "@heliuslabs/zolana/wallet";
import type { Bytes32 } from "@heliuslabs/zolana/keypair";
import { connectClient } from "../../../lib/client";
import { deriveAdapterAuthority } from "../../../lib/deriveAuthority";
import { submitFactory } from "../../../lib/send";
import { walletAdapterSigner } from "../../../lib/walletAdapterSigner";
import {
  DEPOSIT_AMOUNT,
  depositSol,
  TRANSFER_AMOUNT,
  transferSol,
  WITHDRAW_AMOUNT,
  withdrawSol,
} from "../../useDeposit";
import type { AdapterWalletAuthority } from "../../../lib/deriveAuthority";
import type { PrivateWalletContext } from "../../usePrivateWallet";

config({ path: resolve(process.cwd(), "../../.env") });
config({ path: resolve(process.cwd(), "../../../rust-client/.env") });
config({ path: resolve(process.cwd(), ".env") });

function loadKeypair(): Keypair {
  const raw = readFileSync(`${homedir()}/.config/solana/id.json`, "utf8");
  return Keypair.fromSecretKey(Uint8Array.from(JSON.parse(raw)));
}

function seed32(keypair: Keypair): Uint8Array {
  return keypair.secretKey.slice(0, 32);
}

function rpcUrl(): string {
  return (
    process.env.ZOLANA_ENDPOINT ||
    process.env.VITE_ZOLANA_ENDPOINT ||
    `https://devnet.helius-rpc.com/?api-key=${process.env.API_KEY || process.env.VITE_API_KEY}`
  );
}

async function contextFor(
  keypair: Keypair,
): Promise<PrivateWalletContext> {
  const client = await connectClient();
  const owner = address(keypair.publicKey.toBase58());
  const ed25519Pk = Uint8Array.from(
    getAddressEncoder().encode(owner),
  ) as Bytes32;
  const seed = seed32(keypair);
  const authority: AdapterWalletAuthority = await deriveAdapterAuthority({
    solanaPublicKey: owner,
    ed25519PublicKey: ed25519Pk,
    signMessage: async (message) => ed25519.sign(message, seed),
  });
  const signer = walletAdapterSigner({
    address: owner,
    signTransaction: async (tx) => {
      tx.sign([keypair]);
      return tx;
    },
  });
  const submit = submitFactory(client, signer);
  const wallet = new Wallet({
    identity: await authority.shieldedAddress(),
  });
  if (!(await isWalletRegistered({ rpc: client, owner }))) {
    const registration = await buildRegistrationTransaction({
      client,
      owner,
      address: await authority.shieldedAddress(),
    });
    if (registration) await submit(registration);
  }
  await syncWallet({ client, wallet, authority });
  return { authority, wallet, submit, client };
}

const ENABLED = Boolean(
  process.env.API_KEY ||
    process.env.VITE_API_KEY ||
    process.env.ZOLANA_ENDPOINT ||
    process.env.VITE_ZOLANA_ENDPOINT,
);

describe.runIf(ENABLED)("e2e signing (devnet)", () => {
  it("deposits, transfers, and withdraws with a keypair stand-in", async () => {
    const payer = loadKeypair();
    const ctx = await contextFor(payer);
    const sol = await ctx.client.getBalance(ctx.authority.solanaPublicKey());
    expect(sol).toBeGreaterThan(1_500_000_000n);

    const recipientKey = Keypair.generate();
    const connection = new Connection(rpcUrl(), "confirmed");
    const fund = new Transaction().add(
      SystemProgram.transfer({
        fromPubkey: payer.publicKey,
        toPubkey: recipientKey.publicKey,
        lamports: 50_000_000,
      }),
    );
    fund.feePayer = payer.publicKey;
    fund.recentBlockhash = (await connection.getLatestBlockhash()).blockhash;
    fund.sign(payer);
    const sig = await connection.sendRawTransaction(fund.serialize());
    await connection.confirmTransaction(sig, "confirmed");
    await contextFor(recipientKey);

    const afterDeposit = await depositSol(ctx);
    expect(afterDeposit.privateBalance).toBe(DEPOSIT_AMOUNT);

    const afterTransfer = await transferSol(
      ctx,
      address(recipientKey.publicKey.toBase58()),
    );
    expect(afterTransfer.privateBalance).toBe(
      DEPOSIT_AMOUNT - TRANSFER_AMOUNT,
    );

    const afterWithdraw = await withdrawSol(ctx);
    expect(afterWithdraw.privateBalance).toBe(
      DEPOSIT_AMOUNT - TRANSFER_AMOUNT - WITHDRAW_AMOUNT,
    );
  });
});
