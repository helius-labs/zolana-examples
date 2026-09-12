import { buildRegistrationTransaction } from "@heliuslabs/zolana";
import { checkRegistration } from "../lib/registration";
import type { PrivateWalletContext } from "../lib/walletContext";

/** Reuses only a verified, matching registration; never overwrites another identity. */
export async function registerPrivateWallet(
  ctx: Pick<
    PrivateWalletContext,
    "client" | "owner" | "keys" | "submit" | "assertActive" | "signal"
  >
) {
  ctx.assertActive();
  const identity = ctx.keys.address();
  const input = { rpc: ctx.client, owner: ctx.owner };
  const request = { signal: ctx.signal };
  const registered = await checkRegistration(input, identity, request);
  ctx.assertActive();
  if (registered) return;
  const transaction = await buildRegistrationTransaction(
    { client: ctx.client, owner: ctx.owner, address: identity },
    request
  );
  ctx.assertActive();
  if (transaction) await ctx.submit(transaction);
  ctx.assertActive();
  const confirmed = await checkRegistration(input, identity, request);
  ctx.assertActive();
  if (!confirmed) throw new Error("Registration is not confirmed. Try again.");
}
