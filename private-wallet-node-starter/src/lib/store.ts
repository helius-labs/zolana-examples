import { mkdir, readFile, rename, writeFile } from "node:fs/promises";
import { join } from "node:path";
import type { webcrypto } from "node:crypto";
import type { SealedSeed, ShieldedIdentity } from "@zolana/tvc-wallet";
import type { WalletDescriptor } from "@zolana/tvc-wallet/protocol";
import { DATA_DIR } from "./config.js";
import type { OwnerKey } from "./turnkey.js";

/**
 * Everything the server keeps for one wallet. `ownerKey` and `clientKey` are
 * secrets; the rest is public. A real deployment keeps the secrets in a KMS
 * or secret store, not in files.
 */
export type StoredWallet = {
  readonly address: string;
  readonly organizationId: string;
  readonly walletId: string;
  /** The root user of the wallet's Turnkey sub-organization. */
  readonly ownerKey: OwnerKey;
  /** The P-256 key that signs this server's requests to the enclave. */
  readonly clientKey: webcrypto.JsonWebKey;
  /** The private-wallet API's grant for `clientKey`. */
  readonly descriptor: WalletDescriptor;
  readonly identity: ShieldedIdentity;
  readonly sealedSeed: SealedSeed;
  /**
   * The slot of the wallet's last transaction, as a decimal string. The
   * indexer must have reached it before the wallet's private state is read.
   */
  readonly lastSlot?: string;
};

const walletPath = (walletAddress: string) =>
  join(DATA_DIR, "wallets", `${walletAddress}.json`);

export async function saveWallet(wallet: StoredWallet): Promise<void> {
  await mkdir(join(DATA_DIR, "wallets"), { recursive: true, mode: 0o700 });
  await writeFile(walletPath(wallet.address), JSON.stringify(wallet, null, 2), {
    mode: 0o600,
    flag: "wx",
  });
}

/** Records the slot the wallet's latest transaction landed in. */
export async function rememberSlot(
  walletAddress: string,
  slot: bigint,
): Promise<void> {
  const stored = await loadWallet(walletAddress);
  if (!stored) throw new Error(`no stored wallet ${walletAddress}`);
  const path = walletPath(walletAddress);
  await writeFile(
    `${path}.tmp`,
    JSON.stringify({ ...stored, lastSlot: String(slot) }, null, 2),
    { mode: 0o600 },
  );
  await rename(`${path}.tmp`, path);
}

export async function loadWallet(
  walletAddress: string,
): Promise<StoredWallet | undefined> {
  if (!/^[1-9A-HJ-NP-Za-km-z]{32,44}$/.test(walletAddress)) return undefined;
  try {
    return JSON.parse(
      await readFile(walletPath(walletAddress), "utf8"),
    ) as StoredWallet;
  } catch {
    return undefined;
  }
}
