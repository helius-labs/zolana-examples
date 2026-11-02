import { createRoot } from "react-dom/client";
import { PrivyProvider } from "@privy-io/react-auth";
import { createSolanaRpc, createSolanaRpcSubscriptions } from "@solana/kit";
import { App } from "./App.js";
import "./style.css";

const appId = import.meta.env.VITE_PRIVY_APP_ID;
const root = createRoot(document.getElementById("root")!);
if (!appId) {
  root.render(
    <main>
      <h1>Privy confidential transfers</h1>
      <p>Set VITE_PRIVY_APP_ID and API_KEY in .env, then restart the development server.</p>
    </main>,
  );
} else {
  root.render(
    <PrivyProvider
      appId={appId}
      config={{
        appearance: { walletChainType: "solana-only" },
        embeddedWallets: { solana: { createOnLogin: "all-users" } },
        solana: {
          rpcs: {
            "solana:devnet": {
              rpc: createSolanaRpc(`${location.origin}/rpc`),
              rpcSubscriptions: createSolanaRpcSubscriptions(
                `${location.origin.replace(/^http/, "ws")}/rpc`,
              ),
            },
          },
        },
      }}
    >
      <App />
    </PrivyProvider>,
  );
}
