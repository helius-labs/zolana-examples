import { afterEach, expect, it, vi } from "vitest";
import { withTimeout } from "./withTimeout";
afterEach(() => vi.useRealTimers());
it("times out a hung read so refresh can recover", async () => {
  vi.useFakeTimers();
  const result = withTimeout(new Promise<never>(() => {}));
  const rejected = expect(result).rejects.toThrow("Balance request timed out");
  await vi.advanceTimersByTimeAsync(15_000);
  await rejected;
  expect(vi.getTimerCount()).toBe(0);
});
it("clears its timer when a read succeeds", async () => {
  vi.useFakeTimers();
  await expect(withTimeout(Promise.resolve(42n))).resolves.toBe(42n);
  expect(vi.getTimerCount()).toBe(0);
});
