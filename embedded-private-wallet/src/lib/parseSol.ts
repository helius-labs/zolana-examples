const MAX_LAMPORTS = (1n << 64n) - 1n;

export function assertLamports(amount: bigint): void {
  if (amount <= 0n) throw new Error("Enter an amount greater than zero.");
  if (amount > MAX_LAMPORTS) throw new Error("This amount is too large.");
}

/** Convert decimal SOL to lamports exactly, without floating-point rounding. */
export function parseSol(value: string): bigint {
  const text = value.trim();
  if (!text) throw new Error("Enter an amount.");
  if (!/^(?:\d+(?:\.\d*)?|\.\d+)$/.test(text)) {
    throw new Error("Enter a valid SOL amount.");
  }
  const [whole, fraction = ""] = text.split(".");
  if (fraction.length > 9) throw new Error("Use up to 9 decimal places.");
  if (whole.length > 20) throw new Error("This amount is too large.");
  const lamports = BigInt(whole || "0") * 1_000_000_000n + BigInt(fraction.padEnd(9, "0"));
  assertLamports(lamports);
  return lamports;
}
