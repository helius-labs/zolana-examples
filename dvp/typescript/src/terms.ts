import { getAddressEncoder, getAddressDecoder, address, type Address } from "@solana/kit";
import {
  ShieldedAddress,
  CompressedShieldedAddress,
  poseidon,
  solanaOwnerIdentity,
  type Bytes32,
} from "@heliuslabs/zolana/keypair";
import { assetField, type AssetRegistry } from "@heliuslabs/zolana/transaction";
import type { SwapDvpTerms } from "./wasm.js";
export const PROGRAM_ID = address("8ErdEhvg1NgRzzZRSw3Lx4z4rDEruc6Bjwzn7WVooUMx");
export const ZERO = new Uint8Array(32) as Bytes32;
const addressEncoder = getAddressEncoder();
export const encoder = {
  encode: (value: Address): Uint8Array => Uint8Array.from(addressEncoder.encode(value)),
};
export const decoder = getAddressDecoder();
export function bytes32(value: Uint8Array): Bytes32 {
  if (value.length !== 32) throw new Error("Expected 32 bytes");
  return value as Bytes32;
}
export function field(value: bigint): Bytes32 {
  if (
    value < 0n ||
    value >= 21888242871839275222246405745257275088548364400416034343698204186575808495617n
  )
    throw new Error("Non-canonical field");
  return bytes32(Uint8Array.from(Buffer.from(value.toString(16).padStart(64, "0"), "hex")));
}
export const h = (...values: readonly Uint8Array[]): Bytes32 => bytes32(poseidon(values));
export const integer = (bytes: Uint8Array): bigint =>
  BigInt(`0x${Buffer.from(bytes).toString("hex") || "0"}`);
export function concat(...parts: readonly Uint8Array[]): Uint8Array {
  return Uint8Array.from(Buffer.concat(parts));
}
export function hashBytes(value: Uint8Array): Bytes32 {
  let result = ZERO;
  for (let offset = 0; offset < value.length; offset += 31) {
    const next = field(integer(value.slice(offset, offset + 31)));
    result = offset === 0 ? next : h(result, next);
  }
  return result;
}
export function u64(value: bigint): Uint8Array {
  if (value < 0n || value > 0xffffffffffffffffn) throw new Error("Value outside u64");
  const bytes = new Uint8Array(8);
  new DataView(bytes.buffer).setBigUint64(0, value, true);
  return bytes;
}
export interface SwapDvp {
  seller: Address;
  buyer: Address;
  deliveryMint: Address;
  paymentMint: Address;
  settlementAuthority: Address;
  deliveryAmount: bigint;
  paymentAmount: bigint;
  expiryTimestamp: bigint;
  nonce: bigint;
  refString: Uint8Array;
  sellerSettlementDestination: ShieldedAddress;
  buyerSettlementDestination: ShieldedAddress;
  earliestSettlementTimestamp?: bigint;
  sellerRefundDestination: ShieldedAddress;
  buyerRefundDestination: ShieldedAddress;
  creator: Address;
  blinding: Bytes32;
}
export function circuitTerms(t: SwapDvp, assets: AssetRegistry): SwapDvpTerms {
  const destination = (value: ShieldedAddress) => ({
    address: value.toBytes(),
    viewingKeyHash: hashBytes(value.viewingPublicKey.toBytes()),
  });
  return {
    sellerIdentity: solanaOwnerIdentity(encoder.encode(t.seller)),
    buyerIdentity: solanaOwnerIdentity(encoder.encode(t.buyer)),
    settlementAuthorityIdentity: solanaOwnerIdentity(encoder.encode(t.settlementAuthority)),
    creatorIdentity: solanaOwnerIdentity(encoder.encode(t.creator)),
    deliveryMint: { asset: t.deliveryMint, assetId: assets.assetId(t.deliveryMint) },
    paymentMint: { asset: t.paymentMint, assetId: assets.assetId(t.paymentMint) },
    deliveryAmount: t.deliveryAmount,
    paymentAmount: t.paymentAmount,
    expiryTimestamp: t.expiryTimestamp,
    nonce: t.nonce,
    referenceHash: hashBytes(t.refString),
    hasEarliestSettlement: t.earliestSettlementTimestamp !== undefined,
    earliestSettlementTimestamp: t.earliestSettlementTimestamp ?? 0n,
    blinding: t.blinding,
    sellerSettlementDestination: destination(t.sellerSettlementDestination),
    buyerSettlementDestination: destination(t.buyerSettlementDestination),
    sellerRefundDestination: destination(t.sellerRefundDestination),
    buyerRefundDestination: destination(t.buyerRefundDestination),
  };
}

export function dvpId(t: SwapDvp): Bytes32 {
  return h(
    h(
      ...[t.seller, t.buyer, t.settlementAuthority, t.creator].map((value) =>
        solanaOwnerIdentity(encoder.encode(value)),
      ),
    ),
    h(
      assetField(t.deliveryMint),
      assetField(t.paymentMint),
      field(t.deliveryAmount),
      field(t.paymentAmount),
    ),
    h(
      field(t.expiryTimestamp),
      field(BigInt(t.earliestSettlementTimestamp !== undefined)),
      field(t.earliestSettlementTimestamp ?? 0n),
      field(t.nonce),
    ),
    h(
      ...[
        t.sellerSettlementDestination,
        t.buyerSettlementDestination,
        t.sellerRefundDestination,
        t.buyerRefundDestination,
      ].map((value) => CompressedShieldedAddress.fromAddress(value).hash()),
    ),
    hashBytes(t.refString),
    t.blinding,
  );
}
export function validateTerms(t: SwapDvp): void {
  if (t.deliveryAmount <= 0n || t.paymentAmount <= 0n || t.deliveryMint === t.paymentMint)
    throw new Error("Invalid DvP legs");
  for (const amount of [t.deliveryAmount, t.paymentAmount, t.nonce]) u64(amount);
  if (t.refString.length !== 64) throw new Error("refString must be 64 bytes");
  for (const timestamp of [t.expiryTimestamp, t.earliestSettlementTimestamp ?? 0n])
    if (timestamp < 0n || timestamp >= 1n << 63n)
      throw new Error("Timestamp outside non-negative i64");
  if ((t.earliestSettlementTimestamp ?? 0n) > t.expiryTimestamp)
    throw new Error("Earliest settlement follows expiry");
  for (const a of [
    t.sellerSettlementDestination,
    t.buyerSettlementDestination,
    t.sellerRefundDestination,
    t.buyerRefundDestination,
  ])
    a.solanaAddress();
  dvpId(t);
}
export function encodeTerms(t: SwapDvp): Uint8Array {
  validateTerms(t);
  return concat(
    ...[t.seller, t.buyer, t.deliveryMint, t.paymentMint, t.settlementAuthority].map((a) =>
      encoder.encode(a),
    ),
    ...[t.deliveryAmount, t.paymentAmount, t.expiryTimestamp, t.nonce].map(u64),
    t.refString,
    t.sellerSettlementDestination.toBytes(),
    t.buyerSettlementDestination.toBytes(),
    Uint8Array.of(t.earliestSettlementTimestamp === undefined ? 0 : 1),
    u64(t.earliestSettlementTimestamp ?? 0n),
    t.sellerRefundDestination.toBytes(),
    t.buyerRefundDestination.toBytes(),
    encoder.encode(t.creator),
    t.blinding,
  );
}
export function decodeTerms(bytes: Uint8Array): SwapDvp {
  let offset = 0;
  const take = (length: number) => {
    const part = bytes.slice(offset, offset + length);
    offset += length;
    if (part.length !== length) throw new Error("Truncated terms");
    return part;
  };
  const address = () => decoder.decode(take(32));
  const number = () => {
    const b = take(8);
    return new DataView(b.buffer, b.byteOffset, 8).getBigUint64(0, true);
  };
  const shielded = () => ShieldedAddress.fromBytes(take(99));
  const seller = address(),
    buyer = address(),
    deliveryMint = address(),
    paymentMint = address(),
    settlementAuthority = address();
  const deliveryAmount = number(),
    paymentAmount = number(),
    expiryTimestamp = number(),
    nonce = number(),
    refString = take(64);
  const sellerSettlementDestination = shielded(),
    buyerSettlementDestination = shielded();
  const flag = take(1)[0],
    earliest = number();
  if ((flag !== 0 && flag !== 1) || (flag === 0 && earliest !== 0n))
    throw new Error("Non-canonical earliest timestamp");
  const result: SwapDvp = {
    seller,
    buyer,
    deliveryMint,
    paymentMint,
    settlementAuthority,
    deliveryAmount,
    paymentAmount,
    expiryTimestamp,
    nonce,
    refString,
    sellerSettlementDestination,
    buyerSettlementDestination,
    ...(flag === 1 ? { earliestSettlementTimestamp: earliest } : {}),
    sellerRefundDestination: shielded(),
    buyerRefundDestination: shielded(),
    creator: address(),
    blinding: bytes32(take(32)),
  };
  if (offset !== bytes.length) throw new Error("Trailing terms bytes");
  validateTerms(result);
  return result;
}
