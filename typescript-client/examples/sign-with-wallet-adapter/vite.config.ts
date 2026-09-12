import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";
import { nodePolyfills } from "vite-plugin-node-polyfills";

export default defineConfig({
  plugins: [react(), nodePolyfills({ include: ["buffer"] })],
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
