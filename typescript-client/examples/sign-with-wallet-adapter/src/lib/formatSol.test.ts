import { describe, expect, it } from "vitest";
import { formatSol } from "./formatSol";
describe("SOL formatting", () => {
  it.each([
    [0n, "0"],
    [1n, "0.000000001"],
    [10_000_000n, "0.01"],
    [3_000_000n, "0.003"],
    [1_000_000_000n, "1"],
    [12_345_678_901n, "12.345678901"],
    [9_007_199_254_740_993n, "9007199.254740993"],
    [-1n, "-0.000000001"],
  ])("formats %s lamports without losing precision", (value, expected) => {
    expect(formatSol(value)).toBe(expected);
  });
});
