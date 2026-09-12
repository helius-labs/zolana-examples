import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";
import { nodePolyfills } from "vite-plugin-node-polyfills";

import { backendProxy, localBackendGuard } from "./src/lib/devProxy";

export default defineConfig({
  plugins: [
    localBackendGuard(),
    react(),
    nodePolyfills({ include: ["buffer"] }),
  ],
  server: { host: "127.0.0.1", proxy: backendProxy() },
  preview: { host: "127.0.0.1", proxy: backendProxy() },
  optimizeDeps: {
    exclude: ["@lightprotocol/hasher.rs"],
    include: ["@lightprotocol/hasher.rs > bn.js"],
  },
  test: {
    environment: "node",
    include: ["src/**/*.test.{ts,tsx}"],
    exclude: ["src/**/integration/**"],
  },
});
