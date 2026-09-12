import {
  loadRecord,
  saveRecord,
  parsePersistentBrowserTvcWalletState,
  parseShieldedIdentity,
  type PersistentBrowserTvcWalletState,
  type PersistentBrowserTvcAuthorizer,
} from "@zolana/tvc-wallet/browser";
import type { ShieldedIdentity } from "@zolana/tvc-wallet";
import type { HeliusWalletSessionWallet } from "helius-wallet-kit";
import { privacyWalletTrustMaterial } from "./tvc-policy";

export function walletStorageName(
  parent: string,
  wallet: HeliusWalletSessionWallet,
) {
  return [
    "zolana-react-tvc-v1",
    parent,
    wallet.organizationId,
    wallet.walletId,
    wallet.walletAccountId,
    wallet.address,
  ].join(":");
}
export function sameIdentity(a: ShieldedIdentity, b: ShieldedIdentity) {
  return (
    a.solanaAddress === b.solanaAddress &&
    a.shieldedOwnerHash === b.shieldedOwnerHash &&
    a.shieldedViewingPublicKey === b.shieldedViewingPublicKey &&
    a.shieldedNullifierPublicKey === b.shieldedNullifierPublicKey
  );
}
export function validateBinding(
  state: PersistentBrowserTvcWalletState,
  wallet: HeliusWalletSessionWallet,
  authorizer: PersistentBrowserTvcAuthorizer,
) {
  const descriptor = state.walletDescriptor;
  const policy = privacyWalletTrustMaterial().releasePolicy.policy;
  if (
    state.clientKeyId !== authorizer.clientKeyId ||
    descriptor.address !== wallet.address ||
    descriptor.turnkey_wallet_id !== wallet.walletId ||
    descriptor.turnkey_organization_id !== wallet.organizationId ||
    descriptor.security_domain_id !== policy.securityDomainId ||
    descriptor.environment !== policy.environment ||
    !Array.isArray(descriptor.allowed_clients) ||
    descriptor.allowed_clients.length !== 1 ||
    descriptor.allowed_clients[0]?.client_public_key !==
      authorizer.clientPublicKey ||
    JSON.stringify(descriptor.allowed_clients[0]?.allowed_operations) !==
      JSON.stringify(policy.allowedOperations)
  ) {
    throw new Error(
      "Stored wallet binding changed. Recovery is required; the saved identity has been preserved.",
    );
  }
}
export const loadWalletState = (database: string) =>
  loadRecord(database, "wallet", parsePersistentBrowserTvcWalletState);
export const loadKnownIdentity = (database: string) =>
  loadRecord(database, "identity", (value) =>
    value === undefined ? null : parseShieldedIdentity(value),
  );
export async function saveWalletState(
  database: string,
  state: PersistentBrowserTvcWalletState,
  assertActive: () => void,
) {
  parsePersistentBrowserTvcWalletState(state);
  const known = await loadKnownIdentity(database);
  assertActive();
  if (known && state.identity && !sameIdentity(known, state.identity))
    throw new Error("ShieldedIdentityChanged");
  // Persist the public identity first. A partial write may require bootstrap again,
  // but can never silently replace the identity previously observed by this device.
  if (state.identity) await saveRecord(database, "identity", state.identity);
  assertActive();
  await saveRecord(database, "wallet", state);
  assertActive();
}
