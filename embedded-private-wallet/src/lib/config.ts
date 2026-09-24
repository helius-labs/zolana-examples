const nodeEnv = typeof process === "undefined" ? {} : process.env;

export function getRpcEndpoint(): string | undefined {
  const endpoint =
    import.meta.env.VITE_ZOLANA_ENDPOINT ||
    nodeEnv.VITE_ZOLANA_ENDPOINT ||
    nodeEnv.ZOLANA_ENDPOINT;
  if (endpoint) return endpoint;
  const apiKey =
    import.meta.env.VITE_API_KEY || nodeEnv.VITE_API_KEY || nodeEnv.API_KEY;
  if (!apiKey || apiKey === "YOUR_KEY") return undefined;
  return `https://devnet.helius-rpc.com/?api-key=${encodeURIComponent(apiKey)}`;
}
