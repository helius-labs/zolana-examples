import { createZolanaClient } from "@heliuslabs/zolana";
import { getRpcEndpoint } from "./config";

const INDEXER_URL =
  "http://zolnet-devnet-1779374825.eu-north-1.elb.amazonaws.com";
const PROVER_URL =
  "http://zolnet-devnet-1779374825.eu-north-1.elb.amazonaws.com:3001";

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
      INDEXER_URL,
    proverUrl:
      import.meta.env.VITE_ZOLANA_PROVER_URL ||
      nodeEnv.VITE_ZOLANA_PROVER_URL ||
      nodeEnv.ZOLANA_PROVER_URL ||
      PROVER_URL,
    allowInsecureHttp: true,
  });
}
