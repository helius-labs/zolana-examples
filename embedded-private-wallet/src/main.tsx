import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { HeliusWalletProvider } from "helius-wallet-kit";
import App from "./App";
import { getRpcEndpoint } from "./lib/config";
import "@turnkey/react-wallet-kit/styles.css";
import "./index.css";

function Root() {
  const apiKey = import.meta.env.VITE_API_KEY?.trim();
  if (!getRpcEndpoint() || !apiKey || apiKey === "YOUR_KEY") {
    return (
      <main className="wallet-page">
        <section className="wallet-panel wallet-shell" role="alert">
          <h1>Wallet not configured</h1>
          <p className="help-text">
            Set VITE_API_KEY in this example’s .env to a Helius project with
            embedded wallets enabled, then restart the dev server.
          </p>
        </section>
      </main>
    );
  }
  return (
    <HeliusWalletProvider
      config={{
        apiKey,
        cluster: "devnet",
        theme: { darkMode: false, primaryColor: "#e84125", borderRadius: "20px", logoLight: "/wallet-mark.svg", logoDark: "/wallet-mark.svg" },
        secureRpcUrl: {},
        authMethods: { email: true, wallet: false, passkey: false },
      }}
    >
      <App />
    </HeliusWalletProvider>
  );
}

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <Root />
  </StrictMode>,
);
