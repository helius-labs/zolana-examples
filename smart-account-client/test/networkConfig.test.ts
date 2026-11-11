import assert from "node:assert/strict";
import { homedir } from "node:os";
import { describe, it } from "node:test";

import { networkConfig } from "./network/config.js";

describe("network test configuration", () => {
  it("selects the requested network's RPC and the CLI wallet", () => {
    const env = {
      SOLANA_DEVNET_RPC_URL: "https://devnet.helius-rpc.com/?api-key=devnet",
      SOLANA_MAINNET_RPC_URL: "https://mainnet.helius-rpc.com/?api-key=mainnet",
    };
    for (const network of ["devnet", "mainnet"] as const) {
      assert.deepEqual(networkConfig({ ...env, SA_NETWORK: network }), {
        network,
        rpcUrl:
          env[`SOLANA_${network.toUpperCase()}_RPC_URL` as keyof typeof env],
        payerPath: `${homedir()}/.config/solana/id.json`,
      });
    }
  });

  it("requires the selected network's RPC instead of using another network", () => {
    assert.throws(
      () =>
        networkConfig({
          SA_NETWORK: "mainnet",
          SOLANA_DEVNET_RPC_URL: "https://devnet.helius-rpc.com",
        }),
      /set SOLANA_MAINNET_RPC_URL/,
    );
    assert.throws(() => networkConfig({}), /select a network/);
    assert.throws(
      () => networkConfig({ SA_NETWORK: "localnet" }),
      /select a network/,
    );
  });

  it("uses the supplied payer file", () => {
    assert.equal(
      networkConfig({
        SA_NETWORK: "devnet",
        SOLANA_DEVNET_RPC_URL: "https://devnet.helius-rpc.com",
        ZOLANA_PAYER_KEYPAIR: "/tmp/ci-wallet.json",
      }).payerPath,
      "/tmp/ci-wallet.json",
    );
  });
});
