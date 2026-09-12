import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { getRpcEndpoint } from "./config";
beforeEach(() => {
  for (const key of [
    "VITE_API_KEY",
    "API_KEY",
    "VITE_ZOLANA_ENDPOINT",
    "ZOLANA_ENDPOINT",
  ])
    vi.stubEnv(key, "");
});
afterEach(() => vi.unstubAllEnvs());
describe("RPC configuration", () => {
  it("never builds an endpoint with an undefined or placeholder key", () => {
    expect(getRpcEndpoint()).toBeUndefined();
    vi.stubEnv("VITE_API_KEY", "YOUR_KEY");
    expect(getRpcEndpoint()).toBeUndefined();
  });
  it("uses the configured browser key for devnet", () => {
    vi.stubEnv("VITE_API_KEY", "test-key");
    expect(getRpcEndpoint()).toBe(
      "https://devnet.helius-rpc.com/?api-key=test-key",
    );
  });
  it("preserves explicit RPC overrides", () => {
    vi.stubEnv("VITE_ZOLANA_ENDPOINT", "https://rpc.example.test");
    vi.stubEnv("VITE_API_KEY", "test-key");
    expect(getRpcEndpoint()).toBe("https://rpc.example.test");
  });
});
