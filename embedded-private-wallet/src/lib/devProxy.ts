import type { IncomingMessage } from "node:http";
import type { Plugin, ProxyOptions } from "vite";

export const TVC_BACKEND = "https://i6npwfd4mh.eu-west-1.awsapprunner.com";
const allowed = new Map<string, string>([
  ["/api/tvc/v1/info", "GET"],
  ["/api/tvc/v1/ping", "POST"],
  ["/api/tvc/v1/operations", "POST"],
  ["/api/tvc/boot-proof", "POST"],
  ["/api/tvc/enrollment-challenge", "POST"],
  ["/api/tvc/provision-descriptor", "POST"],
  ...[
    "getEncryptedUtxosByTags",
    "getShieldedTransactionsByTags",
    "getShieldedTransactionsByNullifiers",
    "getShieldedTransactionsBySignature",
    "getMerkleProofs",
    "getNonInclusionProofs",
  ].map(
    (method) => [`/api/zolana/indexer/${method}`, "POST"] as [string, string],
  ),
  ["/api/zolana/prover/prove", "POST"],
  ["/api/zolana/prover/prove/status", "GET"],
]);
export function validateProxyRequest(
  req: Pick<IncomingMessage, "url" | "method" | "headers">,
): number | null {
  const url = new URL(req.url ?? "/", "http://localhost");
  if (!url.pathname.startsWith("/api/")) return null;
  if (allowed.get(url.pathname) !== req.method) return 404;
  const host = req.headers.host;
  if (!host || !/^(localhost|127\.0\.0\.1):\d+$/.test(host)) return 403;
  const origin = req.headers.origin;
  if (!origin && req.method !== "GET") return 403;
  if (origin) {
    try {
      const parsed = new URL(origin);
      if (parsed.origin !== `http://${host}`) return 403;
    } catch {
      return 403;
    }
  }
  return 0;
}
export function backendProxy(): Record<string, ProxyOptions> {
  const options: ProxyOptions = {
    target: TVC_BACKEND,
    changeOrigin: true,
    timeout: 95_000,
    proxyTimeout: 95_000,
    configure(proxy) {
      proxy.on("proxyReq", (proxyReq, req) => {
        // Middleware has validated the original host and origin; don't accept a
        // caller-supplied forwarded host or rewrite Origin to bypass server checks.
        proxyReq.setHeader("x-forwarded-host", req.headers.host!);
        proxyReq.setHeader("x-forwarded-proto", "http");
        proxyReq.removeHeader("cookie");
      });
    },
  };
  return { "/api/tvc/": options, "/api/zolana/": options };
}
export function localBackendGuard(): Plugin {
  const install = (server: {
    middlewares: {
      use: (
        handler: (
          req: IncomingMessage,
          res: import("node:http").ServerResponse,
          next: () => void,
        ) => void,
      ) => unknown;
    };
  }) => {
    server.middlewares.use((req, res, next) => {
      const status = validateProxyRequest(req);
      if (status === null || status === 0) {
        next();
        return;
      }
      res.writeHead(status, { "content-type": "application/json" });
      res.end(JSON.stringify({ error: "LocalProxyRequestDenied" }));
    });
  };
  return {
    name: "local-tvc-backend",
    configureServer: install,
    configurePreviewServer: install,
  };
}
