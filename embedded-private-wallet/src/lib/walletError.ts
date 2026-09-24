/** Show SDK cause codes without exposing request bodies, keys, or signatures. */
export function walletError(error: unknown): string {
  const messages: string[] = [];
  const seen = new Set<unknown>();
  let cause = error;
  while (cause && typeof cause === "object" && !seen.has(cause) && seen.size < 5) {
    seen.add(cause);
    const item = cause as { message?: unknown; code?: unknown; causeCode?: unknown; cause?: unknown };
    for (const value of [item.message, item.code, item.causeCode]) {
      if (typeof value === "string" && !messages.includes(value)) messages.push(value);
    }
    cause = item.cause;
  }
  const detail = (messages.join(": ") || "The wallet operation failed. Try again.")
    .replace(/https?:\/\/[^\s)]+/g, "[service URL]")
    .slice(0, 400);
  return messages.includes("WALLET_SYNC")
    ? `Couldn’t sync the private balance. ${detail}`
    : detail;
}
