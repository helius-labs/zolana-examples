import type { Address } from "@solana/kit";
import type { createZolanaClient, WalletKeys } from "@heliuslabs/zolana";
import type { submitFactory } from "./send";

/** Active, verified wallet session shared by the React hooks and operations. */
export type PublicWalletContext = {
  owner: Address;
  submit: ReturnType<typeof submitFactory>;
  client: Awaited<ReturnType<typeof createZolanaClient>>;
  assertActive: () => void;
  signal: AbortSignal;
};

export type PrivateWalletContext = PublicWalletContext & { keys: WalletKeys };
