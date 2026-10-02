/** A positive integer amount of lamports, sent as a string or a number. */
export function lamports(value: unknown): bigint | undefined {
  if (typeof value !== "string" && typeof value !== "number") return undefined;
  if (!/^[1-9][0-9]*$/.test(String(value))) return undefined;
  return BigInt(value);
}
