/**
 * Squads smart-account program: PDAs, the three instructions this example
 * sends, and a decoder for the `Settings` account. Layouts follow the
 * deployed program (Anchor borsh) and the Rust `spr-smart-account-client`.
 */
import {
  AccountRole,
  address,
  getAddressDecoder,
  getAddressEncoder,
  getProgramDerivedAddress,
  mergeRoles,
  type AccountMeta,
  type AccountSignerMeta,
  type Address,
  type Instruction,
  type TransactionSigner,
} from "@solana/kit";

import { ByteReader, ByteWriter, bytesEqual } from "./bytes.js";

export const SMART_ACCOUNT_PROGRAM_ADDRESS = address(
  "SMRTzfY6DfH5ik3TKiyLFfXexV8uSG3d2UksSCYdunG",
);
export const SYSTEM_PROGRAM_ADDRESS = address(
  "11111111111111111111111111111111",
);

// Anchor discriminators: sha256("global:<name>")[0..8] and
// sha256("account:<name>")[0..8].
const CREATE_SMART_ACCOUNT = Uint8Array.of(197, 102, 253, 231, 77, 84, 50, 17);
const EXECUTE_TRANSACTION_SYNC_V2 = Uint8Array.of(
  90,
  81,
  187,
  81,
  39,
  70,
  128,
  78,
);
const EXECUTE_SETTINGS_TRANSACTION_SYNC = Uint8Array.of(
  138,
  209,
  64,
  163,
  79,
  67,
  233,
  76,
);
const SETTINGS_DISCRIMINATOR = Uint8Array.of(
  0xdf,
  0xb3,
  0xa3,
  0xbe,
  0xb1,
  0xe0,
  0x43,
  0xad,
);
export const PROGRAM_CONFIG_DISCRIMINATOR = Uint8Array.of(
  196,
  210,
  90,
  231,
  144,
  149,
  140,
  63,
);

const utf8 = (text: string): Uint8Array => new TextEncoder().encode(text);
const addressEncoder = getAddressEncoder();
const addressDecoder = getAddressDecoder();

/** Squads permission bits. */
export const Permission = Object.freeze({
  Initiate: 1,
  Vote: 2,
  Execute: 4,
  All: 7,
});

export interface SmartAccountSigner {
  readonly key: Address;
  /** Bitmask of `Permission`. */
  readonly permissions: number;
}

export interface Settings {
  /** All zeros for an autonomous account, which this example creates. */
  readonly settingsAuthority: Address;
  readonly threshold: number;
  readonly timeLock: number;
  readonly signers: readonly SmartAccountSigner[];
}

export interface ProgramConfig {
  readonly smartAccountIndex: bigint;
  readonly treasury: Address;
}

export type SettingsAction =
  | Readonly<{ kind: "addSigner"; signer: SmartAccountSigner }>
  | Readonly<{ kind: "removeSigner"; key: Address }>
  | Readonly<{ kind: "changeThreshold"; threshold: number }>;

/** A Squads PDA; every Squads seed list starts with "smart_account". */
async function squadsPda(...seeds: Uint8Array[]): Promise<Address> {
  const [pda] = await getProgramDerivedAddress({
    programAddress: SMART_ACCOUNT_PROGRAM_ADDRESS,
    seeds: [utf8("smart_account"), ...seeds],
  });
  return pda;
}

export function programConfigAddress(): Promise<Address> {
  return squadsPda(utf8("program_config"));
}

/** The `Settings` PDA of the smart account created with `seed`. */
export function settingsAddress(seed: bigint): Promise<Address> {
  return squadsPda(utf8("settings"), new ByteWriter().u128(seed).finish());
}

/** The vault: the smart account PDA of `settings` at index 0. */
export function vaultAddress(settings: Address): Promise<Address> {
  return squadsPda(
    new Uint8Array(addressEncoder.encode(settings)),
    utf8("smart_account"),
    Uint8Array.of(0),
  );
}

export function decodeProgramConfig(data: Uint8Array): ProgramConfig {
  const reader = new ByteReader(data, "Squads ProgramConfig");
  if (
    !bytesEqual(reader.bytes(8, "discriminator"), PROGRAM_CONFIG_DISCRIMINATOR)
  ) {
    throw new Error("not a Squads ProgramConfig account");
  }
  const smartAccountIndex = reader.u128("smart_account_index");
  reader.bytes(32, "authority");
  reader.u64("smart_account_creation_fee");
  const treasury = addressDecoder.decode(reader.bytes(32, "treasury"));
  return { smartAccountIndex, treasury };
}

export function decodeSettings(data: Uint8Array): Settings {
  const reader = new ByteReader(data, "Squads Settings");
  if (!bytesEqual(reader.bytes(8, "discriminator"), SETTINGS_DISCRIMINATOR)) {
    throw new Error("not a Squads Settings account");
  }
  reader.u128("seed");
  const settingsAuthority = addressDecoder.decode(
    reader.bytes(32, "settings_authority"),
  );
  const threshold = reader.u16("threshold");
  const timeLock = reader.u32("time_lock");
  reader.u64("transaction_index");
  reader.u64("stale_transaction_index");
  const archivalTag = reader.u8("archival_authority tag");
  if (archivalTag === 1) {
    reader.bytes(32, "archival_authority");
  } else if (archivalTag !== 0) {
    throw new Error(`unknown archival_authority tag ${archivalTag}`);
  }
  reader.u64("archivable_after");
  reader.u8("bump");
  const count = reader.u32("signers length");
  const signers: SmartAccountSigner[] = [];
  for (let index = 0; index < count; index += 1) {
    const key = addressDecoder.decode(reader.bytes(32, "signer key"));
    const permissions = reader.u8("signer permissions");
    signers.push({ key, permissions });
  }
  // Trailing fields (account utilization, policy data) are not used here.
  return { settingsAuthority, threshold, timeLock, signers };
}

function writeSigner(writer: ByteWriter, signer: SmartAccountSigner): void {
  writer.bytes(addressEncoder.encode(signer.key)).u8(signer.permissions);
}

function signerMeta(
  signer: TransactionSigner,
  role: AccountRole,
): AccountSignerMeta {
  return { address: signer.address, role, signer } as AccountSignerMeta;
}

/**
 * `createSmartAccount` for an autonomous account: no settings authority, no
 * time lock, no rent collector. The new Settings PDA is the last account.
 */
export async function createSmartAccountInstruction(
  input: Readonly<{
    creator: TransactionSigner;
    treasury: Address;
    settings: Address;
    signers: readonly SmartAccountSigner[];
    threshold: number;
  }>,
): Promise<Instruction> {
  const writer = new ByteWriter()
    .bytes(CREATE_SMART_ACCOUNT)
    .u8(0) // settings_authority: None
    .u16(input.threshold)
    .u32(input.signers.length);
  for (const signer of input.signers) writeSigner(writer, signer);
  writer
    .u32(0) // time_lock
    .u8(0) // rent_collector: None
    .u8(0); // memo: None
  return {
    programAddress: SMART_ACCOUNT_PROGRAM_ADDRESS,
    accounts: [
      { address: await programConfigAddress(), role: AccountRole.WRITABLE },
      { address: input.treasury, role: AccountRole.WRITABLE },
      signerMeta(input.creator, AccountRole.WRITABLE_SIGNER),
      { address: SYSTEM_PROGRAM_ADDRESS, role: AccountRole.READONLY },
      { address: SMART_ACCOUNT_PROGRAM_ADDRESS, role: AccountRole.READONLY },
      { address: input.settings, role: AccountRole.WRITABLE },
    ],
    data: writer.finish(),
  };
}

/**
 * Compiles inner instructions into the Squads sync payload. The vault comes
 * first and loses its signer flag on the outer instruction; Squads signs for
 * it in the CPI. Indexes count from the first account after the approvers.
 */
export function compileSyncPayload(
  instructions: readonly Instruction[],
  vault: Address,
): Readonly<{ payload: Uint8Array; accounts: AccountMeta[] }> {
  const accounts: AccountMeta[] = [];
  const indexOf = (meta: AccountMeta): number => {
    const found = accounts.findIndex(
      (existing) => existing.address === meta.address,
    );
    // `accounts[-1]` is undefined, so a new address takes this branch.
    const existing = accounts[found];
    if (existing === undefined) {
      // Payload indexes are u8.
      if (accounts.length === 256)
        throw new Error("sync payload has more than 256 accounts");
      accounts.push(meta);
      return accounts.length - 1;
    }
    accounts[found] = {
      ...existing,
      ...("signer" in meta ? { signer: meta.signer } : {}),
      role: mergeRoles(existing.role, meta.role),
    } as AccountMeta;
    return found;
  };
  indexOf({ address: vault, role: AccountRole.WRITABLE });
  const writer = new ByteWriter().u8(instructions.length);
  for (const instruction of instructions) {
    writer.u8(
      indexOf({
        address: instruction.programAddress,
        role: AccountRole.READONLY,
      }),
    );
    const metas = instruction.accounts ?? [];
    writer.u8(metas.length);
    for (const meta of metas) writer.u8(indexOf(meta));
    const data = instruction.data ?? new Uint8Array();
    writer.u16(data.length).bytes(data);
  }
  // The vault starts writable, so merging never narrows it; only its signer
  // flag is dropped.
  const outer = accounts.map(
    (meta): AccountMeta =>
      meta.address === vault
        ? { address: vault, role: AccountRole.WRITABLE }
        : meta,
  );
  return { payload: writer.finish(), accounts: outer };
}

/**
 * `executeTransactionSyncV2`: Squads checks that `approvers` meet the
 * threshold and permissions, then CPIs `inner` signed by the vault.
 */
export async function executeTransactionSyncInstruction(
  input: Readonly<{
    settings: Address;
    approvers: readonly TransactionSigner[];
    inner: readonly Instruction[];
  }>,
): Promise<Instruction> {
  const vault = await vaultAddress(input.settings);
  const { payload, accounts } = compileSyncPayload(input.inner, vault);
  const data = new ByteWriter()
    .bytes(EXECUTE_TRANSACTION_SYNC_V2)
    .u8(0) // account_index: the vault
    .u8(input.approvers.length)
    .u8(0) // SyncPayload::Transaction
    .u32(payload.length)
    .bytes(payload)
    .finish();
  return {
    programAddress: SMART_ACCOUNT_PROGRAM_ADDRESS,
    accounts: [
      { address: input.settings, role: AccountRole.WRITABLE },
      { address: SMART_ACCOUNT_PROGRAM_ADDRESS, role: AccountRole.READONLY },
      ...input.approvers.map((approver) =>
        signerMeta(approver, AccountRole.READONLY_SIGNER),
      ),
      ...accounts,
    ],
    data,
  };
}

/**
 * `executeSettingsTransactionSync`: Squads checks that `approvers` meet the
 * threshold and permissions, then applies `actions` in order. `rentPayer`
 * covers reallocation when the signer list grows.
 */
export function executeSettingsTransactionSyncInstruction(
  input: Readonly<{
    settings: Address;
    rentPayer: TransactionSigner;
    approvers: readonly TransactionSigner[];
    actions: readonly SettingsAction[];
  }>,
): Instruction {
  const writer = new ByteWriter()
    .bytes(EXECUTE_SETTINGS_TRANSACTION_SYNC)
    .u8(input.approvers.length)
    .u32(input.actions.length);
  for (const action of input.actions) {
    switch (action.kind) {
      case "addSigner":
        writeSigner(writer.u8(0), action.signer);
        break;
      case "removeSigner":
        writer.u8(1).bytes(addressEncoder.encode(action.key));
        break;
      case "changeThreshold":
        writer.u8(2).u16(action.threshold);
        break;
    }
  }
  writer.u8(0); // memo: None
  return {
    programAddress: SMART_ACCOUNT_PROGRAM_ADDRESS,
    accounts: [
      { address: input.settings, role: AccountRole.WRITABLE },
      signerMeta(input.rentPayer, AccountRole.WRITABLE_SIGNER),
      { address: SYSTEM_PROGRAM_ADDRESS, role: AccountRole.READONLY },
      { address: SMART_ACCOUNT_PROGRAM_ADDRESS, role: AccountRole.READONLY },
      ...input.approvers.map((approver) =>
        signerMeta(approver, AccountRole.READONLY_SIGNER),
      ),
    ],
    data: writer.finish(),
  };
}

/**
 * The settings actions that move `current` to the new signer set and
 * threshold, in the order that keeps every intermediate state valid: add,
 * change threshold, remove.
 */
export function rotationActions(
  current: Settings,
  next: readonly SmartAccountSigner[],
  threshold: number,
): SettingsAction[] {
  const currentKeys = new Set(current.signers.map((signer) => signer.key));
  const nextKeys = new Set(next.map((signer) => signer.key));
  const actions: SettingsAction[] = [];
  for (const signer of next) {
    if (!currentKeys.has(signer.key))
      actions.push({ kind: "addSigner", signer });
  }
  if (threshold !== current.threshold)
    actions.push({ kind: "changeThreshold", threshold });
  for (const signer of current.signers) {
    if (!nextKeys.has(signer.key))
      actions.push({ kind: "removeSigner", key: signer.key });
  }
  return actions;
}

/**
 * Checks that `approvers` satisfy Squads' sync consensus: at least
 * `threshold` signers with vote permission, together holding initiate, vote
 * and execute. Also checks the account is autonomous with no time lock, so
 * only this consensus can change its signers.
 */
export function checkSyncConsensus(
  settings: Settings,
  approvers: readonly Address[],
): void {
  // An autonomous account stores the all-zero key, which encodes as the
  // System Program address.
  if (settings.settingsAuthority !== SYSTEM_PROGRAM_ADDRESS)
    throw new Error("the smart account has a settings authority");
  if (settings.timeLock !== 0)
    throw new Error("sync transactions need time_lock 0");
  const seen = new Set<Address>();
  let mask = 0;
  let votes = 0;
  for (const approver of approvers) {
    if (seen.has(approver)) throw new Error(`duplicate approver ${approver}`);
    seen.add(approver);
    const signer = settings.signers.find((entry) => entry.key === approver);
    if (signer === undefined)
      throw new Error(`${approver} is not a Squads signer`);
    mask |= signer.permissions;
    if ((signer.permissions & Permission.Vote) !== 0) votes += 1;
  }
  if ((mask & Permission.All) !== Permission.All) {
    throw new Error("approvers must together hold initiate, vote and execute");
  }
  if (votes < settings.threshold) {
    throw new Error(
      `need ${settings.threshold} voting approvers, got ${votes}`,
    );
  }
}
