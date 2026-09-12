import assert from "node:assert/strict";
import { test } from "vitest";
import { address, getAddressEncoder } from "@solana/kit";
import { ed25519DerivationMessage, type Bytes32 } from "@heliuslabs/zolana/keypair";
import { approveBootstrapActivity, bootstrapWithApproval } from "./bootstrap-approval";

type Api = Parameters<typeof bootstrapWithApproval>[0];
type Activity = Awaited<ReturnType<Api["getActivity"]>>["activity"];
type ApprovalResult = Awaited<ReturnType<Api["approveActivity"]>>;
const expected = {
  organizationId: "org", walletAddress: "4E2agEUkMiuP3ABYbYTYXuU7bYyqPb3uGsLqs7RDd1U5",
  servicePublicKey: "02" + "11".repeat(32),
};
function fixture(id = "activity"): Activity {
  return {
    id, organizationId: "org", status: "ACTIVITY_STATUS_CONSENSUS_NEEDED",
    type: "ACTIVITY_TYPE_SIGN_RAW_PAYLOAD_V2", canApprove: true, fingerprint: "checked-fingerprint",
    createdAt: { seconds: String(Math.floor(Date.now() / 1000)), nanos: "0" },
    intent: { signRawPayloadIntentV2: {
      signWith: expected.walletAddress,
      payload: Buffer.from(ed25519DerivationMessage(Uint8Array.from(
        getAddressEncoder().encode(address(expected.walletAddress)),
      ) as Bytes32)).toString("hex"),
      encoding: "PAYLOAD_ENCODING_HEXADECIMAL", hashFunction: "HASH_FUNCTION_NOT_APPLICABLE",
    } },
    votes: [{ activityId: id, selection: "VOTE_SELECTION_APPROVED",
      publicKey: expected.servicePublicKey, scheme: "SIGNATURE_SCHEME_TK_API_P256" }],
  } as Activity;
}

test("only the checked bootstrap fingerprint is approved; the result is not returned", async () => {
  let approved = false;
  const result = await approveBootstrapActivity({
    getActivity: async (input) => {
      assert.deepEqual(input, { organizationId: "org", activityId: "activity" });
      return { activity: fixture() };
    },
    approveActivity: async (input) => {
      assert.deepEqual(input, { organizationId: "org", fingerprint: "checked-fingerprint" });
      approved = true;
      return {} as ApprovalResult;
    },
  }, expected, "activity");
  assert.equal(approved, true);
  assert.equal(result, undefined);
});

test("a stolen service credential cannot get approval for another message or wallet", async () => {
  const attacks: ((activity: Activity) => void)[] = [
    (a) => { a.intent.signRawPayloadIntentV2!.payload = "010000" + "00".repeat(120); },
    (a) => { a.intent.signRawPayloadIntentV2!.payload += "00"; },
    (a) => { a.intent.signRawPayloadIntentV2!.payload = Buffer.from("TSPP/derive/v1").toString("hex"); },
    (a) => { a.intent.signRawPayloadIntentV2!.signWith = "11111111111111111111111111111111"; },
    (a) => { a.intent.signRawPayloadIntentV2!.hashFunction = "HASH_FUNCTION_SHA256"; },
    (a) => { a.intent.signRawPayloadIntentV2!.encoding = "PAYLOAD_ENCODING_TEXT_UTF8"; },
    (a) => { a.type = "ACTIVITY_TYPE_SIGN_TRANSACTION_V2"; },
    (a) => { a.intent.signTransactionIntentV2 = {} as NonNullable<Activity["intent"]["signTransactionIntentV2"]>; },
    (a) => { a.organizationId = "another-org"; },
    (a) => { a.id = "another-activity"; },
    (a) => { a.votes[0]!.publicKey = "03" + "22".repeat(32); },
    (a) => { a.votes[0]!.activityId = "another-activity"; },
    (a) => { a.votes[0]!.selection = "VOTE_SELECTION_REJECTED"; },
    (a) => { a.status = "ACTIVITY_STATUS_COMPLETED"; },
    (a) => { a.createdAt.seconds = "0"; },
    (a) => { a.createdAt.seconds = "NaN"; },
    (a) => { a.createdAt.seconds = String(Math.floor(Date.now() / 1000) + 120); },
    (a) => { a.canApprove = false; },
    (a) => { a.fingerprint = ""; },
  ];
  for (const attack of attacks) {
    const activity = fixture();
    attack(activity);
    await assert.rejects(approveBootstrapActivity({
      getActivity: async () => ({ activity }),
      approveActivity: async () => { assert.fail("unsafe activity reached approval"); },
    }, expected, "activity"), /Refusing to approve/);
  }
});

test("bootstrap finds and approves one new valid activity using the owner session", async () => {
  const old = fixture("previous");
  const unrelated = fixture("another-wallet");
  unrelated.intent.signRawPayloadIntentV2!.signWith = "11111111111111111111111111111111";
  const transaction = fixture("transaction");
  transaction.intent.signRawPayloadIntentV2!.payload = "010000" + "00".repeat(120);
  let reads = 0;
  let approvals = 0;
  let started = false;
  let finish!: (value: string) => void;
  const operation = new Promise<string>((resolve) => { finish = resolve; });
  const result = await bootstrapWithApproval({
    getActivities: async () => ({ activities: ++reads === 1 ? [old] : [old, unrelated, transaction, fixture()] }),
    getActivity: async ({ activityId }) => {
      assert.equal(activityId, "activity");
      assert.equal(started, true);
      return { activity: fixture() };
    },
    approveActivity: async ({ fingerprint }) => {
      assert.equal(fingerprint, "checked-fingerprint");
      approvals++;
      finish("sealed bootstrap result");
      return {} as ApprovalResult;
    },
  }, expected, async () => { started = true; return operation; });
  assert.equal(result, "sealed bootstrap result");
  assert.equal(approvals, 1);
});

test("ambiguous concurrent bootstraps fail without approving either one", async () => {
  const second = fixture("second");
  let reads = 0;
  let stopped = false;
  await assert.rejects(bootstrapWithApproval({
    getActivities: async () => ({ activities: ++reads === 1 ? [] : [fixture(), second] }),
    getActivity: async () => { assert.fail("ambiguous request was selected"); },
    approveActivity: async () => { assert.fail("ambiguous request was approved"); },
  }, expected, (signal) => new Promise<never>((_resolve, reject) => {
    signal.addEventListener("abort", () => { stopped = true; reject(signal.reason); }, { once: true });
  })), /Ambiguous bootstrap/);
  assert.equal(stopped, true);
});

test("cancellation stops the operation and prevents approval after a delayed activity fetch", async () => {
  const controller = new AbortController();
  let fetched!: () => void;
  const fetching = new Promise<void>((resolve) => { fetched = resolve; });
  let release!: (value: { activity: Activity }) => void;
  const pending = new Promise<{ activity: Activity }>((resolve) => { release = resolve; });
  let reads = 0;
  let stopped = false;
  const result = bootstrapWithApproval({
    getActivities: async () => ({ activities: ++reads === 1 ? [] : [fixture()] }),
    getActivity: async () => { fetched(); return pending; },
    approveActivity: async () => { assert.fail("approval continued after cancellation"); },
  }, expected, (signal) => new Promise<never>((_resolve, reject) => {
    signal.addEventListener("abort", () => { stopped = true; reject(signal.reason); }, { once: true });
  }), controller.signal);
  await fetching;
  controller.abort(new Error("cancelled bootstrap"));
  await assert.rejects(result, /cancelled bootstrap/);
  release({ activity: fixture() });
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(stopped, true);
});

test("a failed operation stops activity discovery without masking its error", async () => {
  let release!: (value: { activities: Activity[] }) => void;
  const pending = new Promise<{ activities: Activity[] }>((resolve) => { release = resolve; });
  let reads = 0;
  await assert.rejects(bootstrapWithApproval({
    getActivities: async () => ++reads === 1 ? { activities: [] } : pending,
    getActivity: async () => { assert.fail("activity fetched after operation failed"); },
    approveActivity: async () => { assert.fail("approval continued after operation failed"); },
  }, expected, async () => { throw new Error("bootstrap rejected"); }), /bootstrap rejected/);
  release({ activities: [fixture()] });
  await new Promise((resolve) => setImmediate(resolve));
});

test("an already cancelled call never starts bootstrap or queries Turnkey", async () => {
  await assert.rejects(bootstrapWithApproval({
    getActivities: async () => { assert.fail("query after cancellation"); },
    getActivity: async () => { assert.fail("query after cancellation"); },
    approveActivity: async () => { assert.fail("approval after cancellation"); },
  }, expected, async () => { assert.fail("started cancelled operation"); },
  AbortSignal.abort(new Error("already cancelled"))), /already cancelled/);
});

test("an incomplete pending-activity page cannot trigger bootstrap or approval", async () => {
  await assert.rejects(bootstrapWithApproval({
    getActivities: async (query) => {
      assert.equal(query?.paginationOptions?.limit, "100");
      return { activities: Array.from({ length: 100 }, () => fixture()) };
    },
    getActivity: async () => { assert.fail("selected from an incomplete list"); },
    approveActivity: async () => { assert.fail("approved from an incomplete list"); },
  }, expected, async () => { assert.fail("started without a complete snapshot"); }), /Too many pending signing activities/);
});
