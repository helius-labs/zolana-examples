import { homedir } from "node:os";

export type Network = "devnet" | "mainnet";

export const GENESIS_HASH = Object.freeze({
  devnet: "EtWTRABZaYq6iMfeYKouRu166VU2xqa1wcaWoxPkrZBG",
  mainnet: "5eykt4UsFv8P8NJdTREpY1vzqKqZKvdpKuc147dw2N9d",
});

export function networkConfig(env: NodeJS.ProcessEnv = process.env): Readonly<{
  network: Network;
  rpcUrl: string;
  payerPath: string;
}> {
  const network = env["SA_NETWORK"];
  if (network !== "devnet" && network !== "mainnet") {
    throw new Error("run test:devnet or test:mainnet to select a network");
  }
  const rpcVariable = `SOLANA_${network.toUpperCase()}_RPC_URL`;
  const rpcUrl = env[rpcVariable]?.trim();
  if (!rpcUrl) throw new Error(`set ${rpcVariable} to a Helius RPC URL`);
  const payerPath =
    env["ZOLANA_PAYER_KEYPAIR"]?.trim() || "~/.config/solana/id.json";
  return {
    network,
    rpcUrl,
    payerPath: payerPath.startsWith("~/")
      ? `${homedir()}/${payerPath.slice(2)}`
      : payerPath,
  };
}
