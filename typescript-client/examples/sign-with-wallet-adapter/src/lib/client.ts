import { createZolanaClient } from "@heliuslabs/zolana";

const RPC_URL = "https://devnet.helius-rpc.com";
const INDEXER_URL =
  "http://zolnet-devnet-1779374825.eu-north-1.elb.amazonaws.com";
const PROVER_URL =
  "http://zolnet-devnet-1779374825.eu-north-1.elb.amazonaws.com:3001";

export async function connectClient() {
  const apiKey =
    import.meta.env.VITE_API_KEY ||
    process.env.VITE_API_KEY ||
    process.env.API_KEY;
  const endpoint =
    import.meta.env.VITE_ZOLANA_ENDPOINT ||
    process.env.VITE_ZOLANA_ENDPOINT ||
    process.env.ZOLANA_ENDPOINT;
  const solanaRpcUrl =
    endpoint || (apiKey ? `${RPC_URL}/?api-key=${apiKey}` : undefined);
  if (!solanaRpcUrl) {
    throw new Error("set VITE_API_KEY or VITE_ZOLANA_ENDPOINT");
  }
  return createZolanaClient({
    solanaRpcUrl,
    indexerUrl:
      import.meta.env.VITE_ZOLANA_INDEXER_URL ||
      process.env.VITE_ZOLANA_INDEXER_URL ||
      process.env.ZOLANA_INDEXER_URL ||
      INDEXER_URL,
    proverUrl:
      import.meta.env.VITE_ZOLANA_PROVER_URL ||
      process.env.VITE_ZOLANA_PROVER_URL ||
      process.env.ZOLANA_PROVER_URL ||
      PROVER_URL,
    allowInsecureHttp: true,
  });
}
