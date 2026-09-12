import { expect, it } from "vitest";
import { parseSol } from "./parseSol";
it.each([
  ["0.01", 10_000_000n], [".000000001", 1n], ["1.", 1_000_000_000n],
  [" 2.123456789 ", 2_123_456_789n], ["18446744073.709551615", (1n << 64n) - 1n],
])("parses %s exactly", (value, expected) => expect(parseSol(value)).toBe(expected));
it.each(["", "0", "-1", "1e3", "NaN", "0.0000000001", "1,000", "18446744073.709551616"])("rejects %s", value => expect(() => parseSol(value)).toThrow());
