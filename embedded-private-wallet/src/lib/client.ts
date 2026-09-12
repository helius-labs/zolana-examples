import { createZolanaClient } from "@heliuslabs/zolana";
import { getRpcEndpoint } from "./config";

export async function connectClient() {
  const nodeEnv = typeof process === "undefined" ? {} : process.env;
  const solanaRpcUrl = getRpcEndpoint();
  if (!solanaRpcUrl) {
    throw new Error("set VITE_API_KEY or VITE_ZOLANA_ENDPOINT");
  }
  return createZolanaClient({
    solanaRpcUrl,
    indexerUrl:
      import.meta.env.VITE_ZOLANA_INDEXER_URL ||
      nodeEnv.VITE_ZOLANA_INDEXER_URL ||
      nodeEnv.ZOLANA_INDEXER_URL ||
      (typeof window === "undefined"
        ? "http://127.0.0.1:5173/api/zolana/indexer"
        : new URL("/api/zolana/indexer", window.location.origin).href),
    proverUrl:
      import.meta.env.VITE_ZOLANA_PROVER_URL ||
      nodeEnv.VITE_ZOLANA_PROVER_URL ||
      nodeEnv.ZOLANA_PROVER_URL ||
      (typeof window === "undefined"
        ? "http://127.0.0.1:5173/api/zolana/prover"
        : new URL("/api/zolana/prover", window.location.origin).href),
    allowInsecureHttp: true,
  });
}
