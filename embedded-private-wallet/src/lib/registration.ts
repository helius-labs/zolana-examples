import { fetchUserRecord } from "@heliuslabs/zolana/wallet";
import type { ShieldedAddress, RequestContext } from "@heliuslabs/zolana";

/** Never overwrite a registered identity with a newly bootstrapped one. */
export async function checkRegistration(
  input: Parameters<typeof fetchUserRecord>[0],
  identity: ShieldedAddress,
  context?: RequestContext,
) {
  const record = await fetchUserRecord(input, context);
  if (!record) return false;
  const same = (a: ArrayLike<number>, b: ArrayLike<number>) =>
    a.length === b.length && Array.from(a).every((byte, i) => byte === b[i]);
  if (
    record.owner !== input.owner ||
    record.ownerP256 ||
    !same(record.nullifierPublicKey, identity.nullifierPublicKey) ||
    !same(record.viewingPublicKey, identity.viewingPublicKey.toBytes())
  ) {
    throw new Error(
      "The onchain registry contains a different private identity. Registration was not changed.",
    );
  }
  return true;
}
