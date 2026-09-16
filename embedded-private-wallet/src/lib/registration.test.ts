import { beforeEach, expect, it, vi } from "vitest";
import { fetchUserRecord } from "@heliuslabs/zolana/wallet";
import { checkRegistration } from "./registration";
vi.mock("@heliuslabs/zolana/wallet", () => ({ fetchUserRecord: vi.fn() }));
const identity = {
  nullifierPublicKey: new Uint8Array(32).fill(1),
  viewingPublicKey: { toBytes: () => new Uint8Array(33).fill(2) },
} as never;
const input = { rpc: {}, owner: "11111111111111111111111111111111" } as never;
beforeEach(() => vi.resetAllMocks());
it("distinguishes absent registration from a service failure", async () => {
  vi.mocked(fetchUserRecord).mockResolvedValue(undefined);
  expect(await checkRegistration(input, identity)).toBe(false);
  vi.mocked(fetchUserRecord).mockRejectedValue(new Error("RPC unavailable"));
  await expect(checkRegistration(input, identity)).rejects.toThrow(
    "RPC unavailable",
  );
});
it("accepts only the expected owner's exact published keys", async () => {
  const record = {
    owner: "11111111111111111111111111111111",
    nullifierPublicKey: new Uint8Array(32).fill(1),
    viewingPublicKey: new Uint8Array(33).fill(2),
  };
  vi.mocked(fetchUserRecord).mockResolvedValue(record as never);
  expect(await checkRegistration(input, identity)).toBe(true);
  for (const change of [
    { owner: "other" },
    { viewingPublicKey: new Uint8Array(33) },
    { nullifierPublicKey: new Uint8Array(32) },
  ]) {
    vi.mocked(fetchUserRecord).mockResolvedValue({
      ...record,
      ...change,
    } as never);
    await expect(checkRegistration(input, identity)).rejects.toThrow(
      "different private identity",
    );
  }
});
