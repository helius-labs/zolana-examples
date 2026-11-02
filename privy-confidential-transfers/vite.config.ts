import { defineConfig, loadEnv } from "vite";

export default defineConfig(({ mode }) => {
  const env = loadEnv(mode, process.cwd(), "");
  return {
    server: {
      proxy: {
        "/rpc": {
          target: "https://devnet.helius-rpc.com",
          changeOrigin: true,
          ws: true,
          rewrite: () => `/?api-key=${encodeURIComponent(env.API_KEY ?? "")}`,
        },
      },
    },
  };
});
