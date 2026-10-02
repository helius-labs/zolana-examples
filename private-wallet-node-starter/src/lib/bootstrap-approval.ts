import { setTimeout as delay } from "node:timers/promises";
import { address, getAddressEncoder } from "@solana/kit";
import {
  ed25519DerivationMessage,
  type Bytes32,
} from "@heliuslabs/zolana/keypair";
import type { TurnkeyApi } from "./turnkey.js";

type Activity = Awaited<ReturnType<TurnkeyApi["getActivity"]>>["activity"];

type Expected = {
  organizationId: string;
  walletAddress: string;
  servicePublicKey: string;
};

/**
 * The fingerprint of `activity` when it is a fresh request from the enclave's
 * key to sign this wallet's derivation message, and nothing else.
 */
function bootstrapFingerprint(
  activity: Activity,
  expected: Expected,
): string | undefined {
  const intent = activity.intent.signRawPayloadIntentV2;
  const derivationMessage = Buffer.from(
    ed25519DerivationMessage(
      Uint8Array.from(
        getAddressEncoder().encode(address(expected.walletAddress)),
      ) as Bytes32,
    ),
  ).toString("hex");
  const ageMs = Date.now() - Number(activity.createdAt.seconds) * 1000;
  const fresh = Number.isFinite(ageMs) && ageMs > -60_000 && ageMs < 60_000;
  const requestedByEnclave = activity.votes.some(
    (vote) =>
      vote.selection === "VOTE_SELECTION_APPROVED" &&
      vote.scheme === "SIGNATURE_SCHEME_TK_API_P256" &&
      vote.publicKey.replace(/^0x/, "").toLowerCase() ===
        expected.servicePublicKey,
  );
  const matches =
    activity.organizationId === expected.organizationId &&
    activity.status === "ACTIVITY_STATUS_CONSENSUS_NEEDED" &&
    activity.type === "ACTIVITY_TYPE_SIGN_RAW_PAYLOAD_V2" &&
    activity.canApprove &&
    fresh &&
    Object.keys(activity.intent).length === 1 &&
    intent?.signWith === expected.walletAddress &&
    intent.encoding === "PAYLOAD_ENCODING_HEXADECIMAL" &&
    intent.hashFunction === "HASH_FUNCTION_NOT_APPLICABLE" &&
    intent.payload === derivationMessage &&
    requestedByEnclave;
  return matches ? activity.fingerprint : undefined;
}

/**
 * Runs `bootstrap` while approving, as the owner, the one Turnkey activity it
 * creates: the enclave's request to sign the wallet's derivation message.
 */
export async function bootstrapWithApproval<T>(
  api: TurnkeyApi,
  expected: Expected,
  bootstrap: (signal: AbortSignal) => Promise<T>,
): Promise<T> {
  const done = new AbortController();
  const signal = AbortSignal.any([done.signal, AbortSignal.timeout(65_000)]);
  const pending = async () => {
    const { activities } = await api.getActivities({
      organizationId: expected.organizationId,
      filterByStatus: ["ACTIVITY_STATUS_CONSENSUS_NEEDED"],
      filterByType: ["ACTIVITY_TYPE_SIGN_RAW_PAYLOAD_V2"],
      paginationOptions: { limit: "100" },
    });
    if (activities.length >= 100) {
      throw new Error("Too many pending signing activities");
    }
    return activities;
  };
  // Never approve an activity from an earlier run.
  const earlier = new Set((await pending()).map((activity) => activity.id));
  const operation = bootstrap(signal).finally(() => done.abort());
  const approval = (async () => {
    while (!signal.aborted) {
      const candidates = (await pending()).filter(
        (activity) =>
          !earlier.has(activity.id) && bootstrapFingerprint(activity, expected),
      );
      if (candidates.length > 1) {
        throw new Error("More than one bootstrap request for this wallet");
      }
      const [candidate] = candidates;
      if (candidate) {
        const { activity } = await api.getActivity({
          organizationId: expected.organizationId,
          activityId: candidate.id,
        });
        const fingerprint = bootstrapFingerprint(activity, expected);
        if (!fingerprint) throw new Error("The bootstrap request changed");
        // The answer holds the derivation signature. Never return or log it.
        await api.approveActivity({
          organizationId: expected.organizationId,
          fingerprint,
        });
        return;
      }
      await delay(250, undefined, { signal });
    }
  })().catch((error: unknown) => {
    if (!done.signal.aborted) throw error;
  });
  return Promise.race([operation, approval.then(() => operation)]);
}
