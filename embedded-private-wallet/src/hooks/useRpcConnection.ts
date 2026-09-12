import { useMemo } from "react";
import { Connection } from "@solana/web3.js";
import { getRpcEndpoint } from "../lib/config";

export function useRpcConnection() {
  const endpoint = getRpcEndpoint();
  return useMemo(() => {
    if (!endpoint) throw new Error("RPC not configured.");
    return new Connection(endpoint, "confirmed");
  }, [endpoint]);
}
