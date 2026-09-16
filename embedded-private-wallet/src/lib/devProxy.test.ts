import { expect, it, vi } from "vitest";
import { backendProxy, validateProxyRequest } from "./devProxy";
const base = {
  method: "POST",
  url: "/api/tvc/provision-descriptor",
  headers: { host: "127.0.0.1:5173", origin: "http://127.0.0.1:5173" },
};
it("allows only the configured local routes and methods", () => {
  expect(validateProxyRequest(base)).toBe(0);
  expect(validateProxyRequest({ ...base, method: "GET" })).toBe(404);
  expect(validateProxyRequest({ ...base, url: "/api/tvc/arbitrary" })).toBe(
    404,
  );
  expect(validateProxyRequest({ ...base, url: "/src/main.tsx" })).toBeNull();
});
it("rejects remote origins, absent POST origins, and spoofed hosts", () => {
  expect(
    validateProxyRequest({
      ...base,
      headers: { ...base.headers, origin: "https://attacker.example" },
    }),
  ).toBe(403);
  expect(
    validateProxyRequest({ ...base, headers: { host: base.headers.host } }),
  ).toBe(403);
  expect(
    validateProxyRequest({
      ...base,
      headers: { ...base.headers, host: "attacker.example:5173" },
    }),
  ).toBe(403);
});
it("forwards the validated local host without rewriting the browser origin", () => {
  const on = vi.fn();
  backendProxy()["/api/tvc/"].configure!({ on } as never, {});
  const forward = on.mock.calls.find(([event]) => event === "proxyReq")![1];
  const headers = new Map([
    ["origin", base.headers.origin],
    ["x-forwarded-host", "spoofed.example"],
    ["cookie", "browser-session"],
  ]);
  forward({
    setHeader: (name: string, value: string) => headers.set(name, value),
    removeHeader: (name: string) => headers.delete(name),
  }, base);
  expect(headers.get("origin")).toBe(base.headers.origin);
  expect(headers.get("x-forwarded-host")).toBe(base.headers.host);
  expect(headers.get("x-forwarded-proto")).toBe("http");
  expect(headers.has("cookie")).toBe(false);
});
