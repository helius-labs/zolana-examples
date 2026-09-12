import { address, getAddressEncoder } from "@solana/kit";
import { ed25519 } from "@noble/curves/ed25519.js";

/** The signature is secret key material; never log it or return it to a service. */
export function verifyDerivationSignature(
  owner: string,
  message: Uint8Array,
  signature: Uint8Array,
): Uint8Array {
  let valid = false;
  try {
    valid = ed25519.verify(
      signature,
      message,
      Uint8Array.from(getAddressEncoder().encode(address(owner))),
    );
  } catch {
    valid = false;
  } finally {
    if (!valid) signature.fill(0);
  }
  if (!valid) {
    throw new Error(
      "Privy did not sign the exact activation message. Private-wallet activation stopped.",
    );
  }
  return signature;
}
