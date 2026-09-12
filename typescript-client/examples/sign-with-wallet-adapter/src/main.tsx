import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { PrivyProvider } from "@privy-io/react-auth";
import { createSolanaRpc, createSolanaRpcSubscriptions } from "@solana/kit";
import App from "./App";
import { getRpcEndpoint } from "./lib/config";
import "./index.css";

function Root() {
  const endpoint = getRpcEndpoint();
  const appId = import.meta.env.VITE_PRIVY_APP_ID?.trim();
  if (!endpoint || !appId || appId === "YOUR_PRIVY_APP_ID") {
    return (
      <main className="wallet-page">
        <section className="wallet-panel wallet-shell" role="alert">
          <h1>{!endpoint ? "RPC not configured" : "Privy not configured"}</h1>
          <p className="help-text">
            {!endpoint
              ? "Set VITE_API_KEY or VITE_ZOLANA_ENDPOINT in this example’s .env, then restart the dev server."
              : "Set VITE_PRIVY_APP_ID in this example’s .env, then restart the dev server. Enable email login and Solana embedded wallets in your Privy app."}
          </p>
        </section>
      </main>
    );
  }
  const wsEndpoint = new URL(endpoint);
  wsEndpoint.protocol = wsEndpoint.protocol === "https:" ? "wss:" : "ws:";
  return (
    <PrivyProvider
      appId={appId}
      config={{
        loginMethods: ["email"],
        appearance: {
          theme: "light",
          accentColor: "#0071e3",
          walletChainType: "solana-only",
        },
        embeddedWallets: { solana: { createOnLogin: "all-users" } },
        solana: {
          rpcs: {
            "solana:devnet": {
              rpc: createSolanaRpc(endpoint),
              rpcSubscriptions: createSolanaRpcSubscriptions(
                wsEndpoint.toString(),
              ),
            },
          },
        },
      }}
    >
      <App />
    </PrivyProvider>
  );
}

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <Root />
  </StrictMode>,
);
