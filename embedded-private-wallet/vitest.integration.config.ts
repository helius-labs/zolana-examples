import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";
import { nodePolyfills } from "vite-plugin-node-polyfills";

export default defineConfig({
  plugins: [react(), nodePolyfills({ include: ["buffer"] })],
  optimizeDeps: {
    exclude: ["@lightprotocol/hasher.rs"],
  },
  test: {
    environment: "node",
    include: ["src/**/integration/**/*.test.ts"],
    testTimeout: 180_000,
    hookTimeout: 180_000,
    sequence: { concurrent: false },
    fileParallelism: false,
  },
});
