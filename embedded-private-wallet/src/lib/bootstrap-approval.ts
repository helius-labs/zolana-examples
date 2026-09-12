import { getAddressEncoder, address } from "@solana/kit";
import { ed25519DerivationMessage, type Bytes32 } from "@heliuslabs/zolana/keypair";
import type { ClientContextType } from "@turnkey/react-wallet-kit";

type Api = Pick<NonNullable<ClientContextType["httpClient"]>, "getActivity" | "getActivities" | "approveActivity">;
type Activity = Awaited<ReturnType<Api["getActivity"]>>["activity"];

export type BootstrapApproval = {
  organizationId: string;
  walletAddress: string;
  servicePublicKey: string;
};

function bootstrapFingerprint(
  activity: Activity,
  expected: BootstrapApproval,
  activityId: string,
): string | undefined {
  const intent = activity.intent.signRawPayloadIntentV2;
  const expectedPayload = Array.from(ed25519DerivationMessage(
    Uint8Array.from(getAddressEncoder().encode(address(expected.walletAddress))) as Bytes32,
  ), (byte) => byte.toString(16).padStart(2, "0")).join("");
  const createdMs = Number(activity.createdAt.seconds) * 1000;
  const nowMs = Date.now();
  if (
    activity.id !== activityId ||
    activity.organizationId !== expected.organizationId ||
    activity.status !== "ACTIVITY_STATUS_CONSENSUS_NEEDED" ||
    activity.type !== "ACTIVITY_TYPE_SIGN_RAW_PAYLOAD_V2" ||
    !activity.canApprove || !activity.fingerprint ||
    !Number.isFinite(createdMs) || createdMs > nowMs + 60_000 || nowMs - createdMs > 60_000 ||
    Object.keys(activity.intent).length !== 1 ||
    !intent || intent.signWith !== expected.walletAddress ||
    intent.encoding !== "PAYLOAD_ENCODING_HEXADECIMAL" ||
    intent.hashFunction !== "HASH_FUNCTION_NOT_APPLICABLE" ||
    intent.payload !== expectedPayload ||
    !activity.votes.some((vote) =>
      vote.activityId === activityId &&
      vote.selection === "VOTE_SELECTION_APPROVED" &&
      vote.scheme === "SIGNATURE_SCHEME_TK_API_P256" &&
      vote.publicKey.replace(/^0x/, "").toLowerCase() === expected.servicePublicKey,
    )
  ) {
    return undefined;
  }
  return activity.fingerprint;
}

/** Approves only the fingerprint of the activity whose full intent was checked. */
export async function approveBootstrapActivity(
  api: Pick<Api, "getActivity" | "approveActivity">,
  expected: BootstrapApproval,
  activityId: string,
  signal?: AbortSignal,
): Promise<void> {
  signal?.throwIfAborted();
  const { activity } = await api.getActivity({ organizationId: expected.organizationId, activityId });
  const fingerprint = bootstrapFingerprint(activity, expected, activityId);
  if (!fingerprint) {
    throw new Error("Refusing to approve: not a fresh bootstrap request from the pinned enclave for this wallet");
  }
  signal?.throwIfAborted();
  // The result can contain the derivation seed. Never return or log it.
  await api.approveActivity({ organizationId: expected.organizationId, fingerprint });
}

/** Watches only while this caller is bootstrapping, and approves at most one new matching activity. */
export async function bootstrapWithApproval<T>(
  api: Api,
  expected: BootstrapApproval,
  start: (signal: AbortSignal) => Promise<T>,
  callerSignal?: AbortSignal,
): Promise<T> {
  const controller = new AbortController();
  const deadline = AbortSignal.any([
    AbortSignal.timeout(65_000), ...(callerSignal ? [callerSignal] : []),
  ]);
  const signal = AbortSignal.any([controller.signal, deadline]);
  signal.throwIfAborted();
  let onAbort!: () => void;
  const stopped = new Promise<never>((_resolve, reject) => {
    onAbort = () => reject(deadline.reason);
    deadline.addEventListener("abort", onAbort, { once: true });
  });
  const query = {
    organizationId: expected.organizationId,
    filterByStatus: ["ACTIVITY_STATUS_CONSENSUS_NEEDED" as const],
    filterByType: ["ACTIVITY_TYPE_SIGN_RAW_PAYLOAD_V2" as const],
    paginationOptions: { limit: "100" },
  };
  const pendingActivities = async () => {
    const { activities } = await api.getActivities(query);
    signal.throwIfAborted();
    // A full page may omit an older candidate. Do not select from an incomplete list.
    if (activities.length >= 100) throw new Error("Too many pending signing activities to select bootstrap safely");
    return activities;
  };
  try {
    // Never approve activities left over from earlier runs, even if they are fresh.
    const previous = await Promise.race([pendingActivities(), stopped]);
    const before = new Set(previous.map((activity) => activity.id));
    signal.throwIfAborted();
    const operation = Promise.resolve().then(() => {
      signal.throwIfAborted();
      return start(signal);
    }).finally(() => controller.abort());
    const approval = (async () => {
      while (!signal.aborted) {
        const activities = await pendingActivities();
        const candidates = activities.filter((activity) =>
          !before.has(activity.id) && bootstrapFingerprint(activity, expected, activity.id),
        );
        if (candidates.length > 1) throw new Error("Ambiguous bootstrap activities; retry with one active bootstrap per wallet");
        const candidate = candidates[0];
        if (candidate) {
          await approveBootstrapActivity(api, expected, candidate.id, signal);
          return;
        }
        await delay(signal);
      }
    })().catch((error: unknown) => {
      // Finishing or failing the operation stops the watcher; preserve that result.
      if (!controller.signal.aborted) throw error;
    });
    return await Promise.race([operation, approval.then(() => operation), stopped]);
  } finally {
    controller.abort();
    deadline.removeEventListener("abort", onAbort);
  }
}

/** Abortable browser timer; remove the listener after either completion path. */
function delay(signal: AbortSignal): Promise<void> {
  signal.throwIfAborted();
  return new Promise((resolve, reject) => {
    const onAbort = () => {
      clearTimeout(timer);
      reject(signal.reason);
    };
    const timer = setTimeout(() => {
      signal.removeEventListener("abort", onAbort);
      resolve();
    }, 250);
    signal.addEventListener("abort", onAbort, { once: true });
  });
}
