import { teePolicyFromJson } from "@heliuslabs/zolana/client";
import type { ZolanaClientConfig } from "@heliuslabs/zolana";
import teePolicy from "../../devnet-e-tee-policy.json";

export const INDEXER_URL = "https://d34xe1hqb2ibg2.cloudfront.net";
export const PROVER_URL = "https://dd73hh473h6ha.cloudfront.net";

export function clientConfig(solanaRpcUrl: string): ZolanaClientConfig {
  return {
    solanaRpcUrl,
    indexerUrl: INDEXER_URL,
    proverUrl: PROVER_URL,
    proofDataSource: "prover",
    proverTee: teePolicyFromJson(teePolicy),
  };
}
