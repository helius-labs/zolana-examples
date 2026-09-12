import type { Address } from "@solana/kit";
import type {
  createZolanaClient,
  Wallet,
  WalletKeys,
} from "@heliuslabs/zolana";
import type { submitFactory } from "./send";

/** Active, verified wallet session shared by the React hooks and operations. */
export type PrivateWalletContext = {
  owner: Address;
  keys: WalletKeys;
  wallet: Wallet;
  submit: ReturnType<typeof submitFactory>;
  client: Awaited<ReturnType<typeof createZolanaClient>>;
  assertActive: () => void;
  signal: AbortSignal;
};
