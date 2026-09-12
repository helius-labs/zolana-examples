import { address, type Address } from "@solana/kit";
import { resolveRegisteredAddress } from "@heliuslabs/zolana/wallet";
import {
  getDepositInstructionAsync,
  DepositAsset,
} from "@heliuslabs/zolana/instructions";
import { initializePoseidon, randomBlinding } from "@heliuslabs/zolana/keypair";
import { assertLamports } from "../../lib/parseSol";
import type { PrivateWalletContext } from "../../lib/walletContext";
import { compileInstructions } from "../../lib/compileInstructions";
import { syncAfterTransaction } from "../../lib/syncAfterTransaction";
import { DEPOSIT_AMOUNT } from "../../lib/amounts";

export async function depositSol(
  ctx: PrivateWalletContext,
  amount = DEPOSIT_AMOUNT,
  recipient: Address = ctx.owner,
) {
  ctx.assertActive();
  assertLamports(amount);
  const destination = address(recipient);
  await initializePoseidon();
  ctx.assertActive();
  const identity = ctx.keys.address();
  if (identity.solanaAddress() !== ctx.owner)
    throw new Error("Private wallet belongs to another account.");
  let recipientIdentity = identity;
  if (destination !== ctx.owner) {
    const registered = await resolveRegisteredAddress(
      { rpc: ctx.client, owner: destination },
      { signal: ctx.signal },
    );
    ctx.assertActive();
    if (!registered)
      throw new Error("Recipient has not registered a private wallet.");
    if (registered.address.solanaAddress() !== destination)
      throw new Error(
        "Registered private wallet does not match the recipient.",
      );
    recipientIdentity = registered.address;
  }
  const instruction = await getDepositInstructionAsync({
    tree: ctx.client.tree,
    depositor: ctx.owner,
    deposits: [
      {
        asset: DepositAsset.sol(),
        amount,
        viewTag: recipientIdentity.confidentialViewTag(),
        recipientOwnerHash: recipientIdentity.ownerHash(),
        blinding: randomBlinding(),
      },
    ],
  });
  const tx = await compileInstructions(ctx, [instruction]);
  ctx.assertActive();
  const { signature, slot } = await ctx.submit(tx);
  const privateBalance = await syncAfterTransaction(ctx, signature, slot);
  return { signature, privateBalance };
}
