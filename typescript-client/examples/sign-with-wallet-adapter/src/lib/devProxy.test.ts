import { expect, it } from "vitest";
import { validateProxyRequest } from "./devProxy";
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
