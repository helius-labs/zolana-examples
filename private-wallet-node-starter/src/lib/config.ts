import "dotenv/config";

function required(name: string): string {
  const value = process.env[name]?.trim();
  if (!value) throw new Error(`set ${name} in .env`);
  return value;
}

export const HELIUS_API_KEY = required("HELIUS_API_KEY");
export const PORT = Number(process.env.PORT ?? 3300);
export const DATA_DIR = process.env.DATA_DIR ?? ".data";

/** The Helius private-wallet API: the enclave that holds each wallet's private keys. */
export const PRIVATE_WALLET_API_URL = `https://d19hyakngvko5w.cloudfront.net/v1/private-wallet?api-key=${HELIUS_API_KEY}`;
/** The domain the owner's enrollment message names. */
export const ENROLLMENT_DOMAIN = "beta-devnet.helius-rpc.com";

export const HELIUS_API_URL = "https://dev-api.helius.xyz/v0";
export const TURNKEY_API_URL = "https://api.turnkey.com";
export const TURNKEY_AUTH_PROXY_URL = "https://authproxy.turnkey.com";

export const SOLANA_RPC_URL = `https://devnet.helius-rpc.com/?api-key=${HELIUS_API_KEY}`;
export const ZOLANA_INDEXER_URL = "https://d2xah7tnhdhcom.cloudfront.net";
export const ZOLANA_PROVER_URL = "https://d21ni15goiip6l.cloudfront.net";
