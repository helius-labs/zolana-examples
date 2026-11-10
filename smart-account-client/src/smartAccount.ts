/**
 * The Create, Register, Rotate and Spend flows of design.md, as instruction
 * builders. The vault (Squads smart account at index 0) owns the shielded
 * UTXOs; Squads signs for it in `executeTransactionSyncV2`. No program of
 * this example runs on chain and no proof checks the key record.
 */
import {
  AccountRole,
  type Address,
  type Instruction,
  type TransactionSigner,
} from "@solana/kit";
import {
  ShieldedAddress,
  ViewingKey,
  X25519PublicKey,
  type Bytes33,
  type ShieldedKeypair,
} from "@heliuslabs/spr/keypair";
import { LocalKeys, type SprClient } from "@heliuslabs/spr/client";
import { getRegisterInstructionAsync } from "@heliuslabs/spr/instructions";
import { getUserRecordAddress } from "@heliuslabs/spr/addresses";
import {
  USER_REGISTRY_PROGRAM_ID,
  transactInstruction,
} from "@heliuslabs/spr/interface";
import {
  AssetRegistry,
  ConfidentialTransfer,
  LocalShieldedKeys,
  ProofInputUtxo,
  encryptConfidentialTransfer,
  type Utxo,
} from "@heliuslabs/spr/transaction";
import { fetchUserRecord } from "@heliuslabs/spr/wallet";

import { ByteWriter } from "./bytes.js";
import {
  buildKeyRecord,
  compareAddresses,
  generateNullifierKey,
  recoverSharedSecrets,
  type KeyRecipient,
  type KeyRecord,
  type OnChainKeyState,
  type SharedSecrets,
} from "./keyRecord.js";
import {
  checkSyncConsensus,
  createSmartAccountInstruction,
  decodeProgramConfig,
  decodeSettings,
  executeSettingsTransactionSyncInstruction,
  executeTransactionSyncInstruction,
  programConfigAddress,
  rotationActions,
  settingsAddress,
  vaultAddress,
  type Settings,
  type SmartAccountSigner,
} from "./squads.js";
import { currentKeyRecord, type KeyRecordStore } from "./store.js";

export type Client = SprClient;

const UPDATE_KEYS = 2;

/** A signer to add: its Squads permissions and its viewing public key. */
export interface NewSigner extends SmartAccountSigner, KeyRecipient {}

/** A smart account member: its Solana signer and its shielded keypair. */
export interface Member {
  readonly signer: TransactionSigner;
  readonly keypair: ShieldedKeypair;
}

export function memberFromKeypair(keypair: ShieldedKeypair): Member {
  return { signer: keypair.toSolanaSigner(), keypair };
}

async function accountData(
  client: Client,
  account: Address,
): Promise<Uint8Array | undefined> {
  const { value } = await client.solanaRpc
    .getAccountInfo(account, { encoding: "base64", commitment: "confirmed" })
    .send();
  if (value === null) return undefined;
  return new Uint8Array(Buffer.from(value.data[0], "base64"));
}

export async function fetchSettings(
  client: Client,
  settings: Address,
): Promise<Settings> {
  const data = await accountData(client, settings);
  if (data === undefined)
    throw new Error(`Squads Settings ${settings} does not exist`);
  return decodeSettings(data);
}

/** The vault's registered viewing public key, or undefined before Register. */
export async function fetchRegisteredViewingKey(
  client: Client,
  vault: Address,
): Promise<Bytes33 | undefined> {
  const record = await fetchUserRecord({ rpc: client, owner: vault });
  return record?.viewingPublicKey;
}

export async function fetchOnChainKeyState(
  client: Client,
  settings: Address,
): Promise<OnChainKeyState> {
  const decoded = await fetchSettings(client, settings);
  const registered = await fetchRegisteredViewingKey(
    client,
    await vaultAddress(settings),
  );
  return {
    settings,
    signers: decoded.signers.map((signer) => signer.key),
    ...(registered === undefined
      ? {}
      : { registeredViewingPublicKey: registered }),
  };
}

/** The vault's shielded identity under the shared keys. */
export function vaultShieldedAddress(
  vault: Address,
  secrets: SharedSecrets,
): ShieldedAddress {
  return ShieldedAddress.forPda({
    pda: vault,
    nullifierPublicKey: secrets.nullifierKey.publicKey(),
    viewingPublicKey: secrets.viewingKey.publicKey(),
  });
}

/**
 * Recovers the shared secrets of the current key record, and the viewing
 * keys of earlier records `member` was in, current first. The earlier keys
 * open UTXOs the vault received before a rotate.
 */
export async function recoverVaultKeys(
  input: Readonly<{
    client: Client;
    store: KeyRecordStore;
    settings: Address;
    member: Member;
  }>,
): Promise<
  Readonly<{
    record: KeyRecord;
    secrets: SharedSecrets;
    viewingKeys: ViewingKey[];
  }>
> {
  const state = await fetchOnChainKeyState(input.client, input.settings);
  const record = await currentKeyRecord(input.store, state);
  const secrets = recoverSharedSecrets(
    record,
    input.member.signer.address,
    input.member.keypair,
  );
  const viewingKeys = [secrets.viewingKey.clone()];
  try {
    const earlier = (await input.store.list(input.settings))
      .filter((candidate) => candidate.epoch < record.epoch)
      .sort((a, b) => Number(b.epoch - a.epoch));
    for (const candidate of earlier) {
      const old = tryRecover(candidate, input.member);
      if (old === undefined) continue;
      old.nullifierKey.destroy();
      const known = viewingKeys.some((key) =>
        key.publicKey().equals(old.viewingKey.publicKey()),
      );
      if (known) old.viewingKey.destroy();
      else viewingKeys.push(old.viewingKey);
    }
  } catch (error) {
    destroySecrets(secrets);
    viewingKeys.forEach((key) => key.destroy());
    throw error;
  }
  return { record, secrets, viewingKeys };
}

/** Undefined when `member` was not a signer of `record`, or its entry is bad. */
function tryRecover(
  record: KeyRecord,
  member: Member,
): SharedSecrets | undefined {
  try {
    return recoverSharedSecrets(record, member.signer.address, member.keypair);
  } catch {
    return undefined;
  }
}

export function destroySecrets(secrets: SharedSecrets): void {
  secrets.viewingKey.destroy();
  secrets.nullifierKey.destroy();
}

/**
 * Rotate step 4 for an approver that stays a signer: checks that its own
 * entry in the new record opens to keys matching the record. Throws on a bad
 * ciphertext; the approver then refuses to co-sign.
 */
export function checkOwnEntry(record: KeyRecord, member: Member): void {
  destroySecrets(
    recoverSharedSecrets(record, member.signer.address, member.keypair),
  );
}

/** The key holders for syncing and proving as the vault. */
export function vaultKeys(
  client: Client,
  vault: Address,
  secrets: SharedSecrets,
  viewingKeys: readonly ViewingKey[],
): Readonly<{
  address: ShieldedAddress;
  shielded: LocalShieldedKeys;
  proving: LocalKeys;
}> {
  const address = vaultShieldedAddress(vault, secrets);
  const keys = { address, viewingKeys, nullifierKey: secrets.nullifierKey };
  const shielded = LocalShieldedKeys.fromKeys(keys);
  try {
    return {
      address,
      shielded,
      proving: LocalKeys.fromKeys(keys, client.proofService),
    };
  } catch (error) {
    shielded.destroy();
    throw error;
  }
}

// ---------------------------------------------------------------------------
// 1. Create
// ---------------------------------------------------------------------------

export interface CreatedAccount {
  readonly settings: Address;
  readonly vault: Address;
  readonly record: KeyRecord;
  /** The `createSmartAccount` instruction; the creator signs it. */
  readonly instruction: Instruction;
}

/**
 * Create steps 3 and 4: generates the shared secrets and the key record, and
 * returns the `createSmartAccount` instruction. The creator uploads `record`
 * to the key record store, then sends the instruction.
 *
 * The Settings address comes from the Squads account counter, so another
 * account created in between makes the instruction fail; build again then.
 */
export async function createSmartAccount(
  input: Readonly<{
    client: Client;
    creator: TransactionSigner;
    signers: readonly NewSigner[];
    threshold: number;
  }>,
): Promise<CreatedAccount> {
  const configData = await accountData(
    input.client,
    await programConfigAddress(),
  );
  if (configData === undefined)
    throw new Error("the Squads ProgramConfig account is missing");
  const config = decodeProgramConfig(configData);
  const settings = await settingsAddress(config.smartAccountIndex + 1n);
  const vault = await vaultAddress(settings);

  const secrets: SharedSecrets = {
    viewingKey: ViewingKey.generate(),
    nullifierKey: generateNullifierKey(),
  };
  try {
    const record = buildKeyRecord({
      settings,
      epoch: 0n,
      secrets,
      recipients: input.signers,
    });
    const instruction = await createSmartAccountInstruction({
      creator: input.creator,
      treasury: config.treasury,
      settings,
      signers: sortedSigners(input.signers),
      threshold: input.threshold,
    });
    return { settings, vault, record, instruction };
  } finally {
    destroySecrets(secrets);
  }
}

function sortedSigners(signers: readonly NewSigner[]): SmartAccountSigner[] {
  return signers
    .map(({ key, permissions }) => ({ key, permissions }))
    .sort((a, b) => compareAddresses(a.key, b.key));
}

// ---------------------------------------------------------------------------
// Create step 6. Register
// ---------------------------------------------------------------------------

/**
 * Squads `executeTransactionSyncV2` with an inner registry `register`
 * signed by the vault. The vault pays the record's rent, so it needs SOL.
 */
export async function registerVaultInstruction(
  input: Readonly<{
    client: Client;
    settings: Address;
    approvers: readonly TransactionSigner[];
    secrets: SharedSecrets;
  }>,
): Promise<Instruction> {
  await checkApprovers(input.client, input.settings, input.approvers);
  const vault = await vaultAddress(input.settings);
  const register = await getRegisterInstructionAsync({
    owner: vault,
    payer: vault,
    nullifierPublicKey: input.secrets.nullifierKey.publicKey(),
    viewingPublicKey: input.secrets.viewingKey.publicKey().toBytes(),
  });
  return executeTransactionSyncInstruction({
    settings: input.settings,
    approvers: input.approvers,
    inner: [register],
  });
}

async function checkApprovers(
  client: Client,
  settings: Address,
  approvers: readonly TransactionSigner[],
): Promise<Settings> {
  const decoded = await fetchSettings(client, settings);
  checkSyncConsensus(
    decoded,
    approvers.map((approver) => approver.address),
  );
  return decoded;
}

/** Registry `update_keys`: replaces the viewing key of `owner`'s record. */
export async function updateKeysInstruction(
  owner: Address,
  viewingPublicKey: Bytes33,
): Promise<Instruction> {
  return {
    programAddress: USER_REGISTRY_PROGRAM_ID,
    accounts: [
      {
        address: await getUserRecordAddress(owner),
        role: AccountRole.WRITABLE,
      },
      { address: owner, role: AccountRole.READONLY_SIGNER },
    ],
    data: new ByteWriter().u8(UPDATE_KEYS).bytes(viewingPublicKey).finish(),
  };
}

// ---------------------------------------------------------------------------
// 2. Rotate
// ---------------------------------------------------------------------------

export interface Rotation {
  readonly record: KeyRecord;
  /** In order: the registry key update (if registered), then the settings change. */
  readonly instructions: readonly Instruction[];
}

/**
 * A kept signer's viewing public key, read from its user registry record.
 * Taking it from the key record store would let the store choose whom the
 * new shared viewing key is encrypted to.
 */
async function registeredViewingKey(
  client: Client,
  signer: Address,
): Promise<X25519PublicKey> {
  const record = await fetchUserRecord({ rpc: client, owner: signer });
  if (record === undefined) {
    throw new Error(`kept signer ${signer} is not in the user registry`);
  }
  return X25519PublicKey.fromBytes(record.viewingPublicKey);
}

/**
 * Rotate steps 2 to 5: recovers the current secrets, encrypts a new shared
 * viewing key and the unchanged nullifier secret to the new signer set, and
 * returns the new record and the rotate instructions. `keep` lists the
 * current signers that stay, whose viewing keys come from the user registry;
 * `add` lists new signers with their viewing keys. After the approvals
 * (`checkOwnEntry`), the rotator uploads `record`, then sends the
 * instructions.
 */
export async function rotateSmartAccount(
  input: Readonly<{
    client: Client;
    store: KeyRecordStore;
    settings: Address;
    rotator: Member;
    approvers: readonly TransactionSigner[];
    keep: readonly Address[];
    add: readonly NewSigner[];
    threshold: number;
  }>,
): Promise<Rotation> {
  const current = await checkApprovers(
    input.client,
    input.settings,
    input.approvers,
  );
  const kept = await Promise.all(
    input.keep.map(async (key): Promise<NewSigner> => {
      const signer = current.signers.find((entry) => entry.key === key);
      if (signer === undefined) {
        throw new Error(`${key} is not a current signer`);
      }
      return {
        key,
        permissions: signer.permissions,
        viewingPublicKey: await registeredViewingKey(input.client, key),
      };
    }),
  );
  const next = [...kept, ...input.add];

  const { record, secrets, viewingKeys } = await recoverVaultKeys({
    client: input.client,
    store: input.store,
    settings: input.settings,
    member: input.rotator,
  });
  viewingKeys.forEach((key) => key.destroy());
  const newSecrets: SharedSecrets = {
    viewingKey: ViewingKey.generate(),
    nullifierKey: secrets.nullifierKey,
  };
  try {
    const newRecord = buildKeyRecord({
      settings: input.settings,
      epoch: record.epoch + 1n,
      secrets: newSecrets,
      recipients: next,
    });

    const vault = await vaultAddress(input.settings);
    const instructions: Instruction[] = [];
    if ((await fetchRegisteredViewingKey(input.client, vault)) !== undefined) {
      instructions.push(
        await executeTransactionSyncInstruction({
          settings: input.settings,
          approvers: input.approvers,
          inner: [
            await updateKeysInstruction(
              vault,
              newRecord.sharedViewingPublicKey,
            ),
          ],
        }),
      );
    }
    const actions = rotationActions(
      current,
      sortedSigners(next),
      input.threshold,
    );
    // A key-only rotate changes no settings; Squads rejects an empty list.
    if (actions.length > 0) {
      instructions.push(
        executeSettingsTransactionSyncInstruction({
          settings: input.settings,
          rentPayer: input.rotator.signer,
          approvers: input.approvers,
          actions,
        }),
      );
    }
    if (instructions.length === 0) {
      throw new Error(
        "an unregistered vault with unchanged settings has nothing to rotate",
      );
    }
    return { record: newRecord, instructions };
  } finally {
    newSecrets.viewingKey.destroy();
    destroySecrets(secrets);
  }
}

// ---------------------------------------------------------------------------
// 3. Spend
// ---------------------------------------------------------------------------

export interface Payment {
  readonly recipient: ShieldedAddress;
  readonly asset: Address;
  readonly amount: bigint;
}

/**
 * Spend steps 3 to 6: builds a confidential transfer from the vault's UTXOs,
 * proves it with the shared nullifier secret, and wraps the SPP `transact`
 * in Squads `executeTransactionSyncV2`. The vault is the owner signer and
 * the SPP payer, so the vault needs SOL for the fees.
 */
export async function spendInstruction(
  input: Readonly<{
    client: Client;
    settings: Address;
    approvers: readonly TransactionSigner[];
    secrets: SharedSecrets;
    viewingKeys: readonly ViewingKey[];
    inputs: readonly Utxo[];
    payments: readonly Payment[];
  }>,
): Promise<Instruction> {
  await checkApprovers(input.client, input.settings, input.approvers);
  const vault = await vaultAddress(input.settings);
  const assets = new AssetRegistry();
  const keys = vaultKeys(input.client, vault, input.secrets, input.viewingKeys);
  try {
    const proofInputs = input.inputs.map((utxo) =>
      ProofInputUtxo.fromNullifierKey(utxo, input.secrets.nullifierKey),
    );
    const transfer = new ConfidentialTransfer(keys.address, proofInputs, vault);
    for (const payment of input.payments) {
      transfer.send(payment.recipient, payment.asset, payment.amount);
    }
    const prepared = transfer.prepare();
    const [txKey] = await keys.shielded.transactionKeys([
      {
        viewingPublicKey: keys.address.viewingPublicKey,
        firstNullifier: prepared.firstNullifier,
      },
    ]);
    if (txKey === undefined)
      throw new Error("no transaction key for the transfer");
    let encrypted: ReturnType<typeof encryptConfidentialTransfer>;
    try {
      encrypted = encryptConfidentialTransfer(txKey, {
        outputs: prepared.outputs,
        assets,
      });
    } finally {
      txKey.destroy();
    }
    const sppProofInputs = prepared.finalize({
      txViewingPublicKey: encrypted.txViewingPublicKey,
      salt: encrypted.salt,
      payload: encrypted.payload,
    });
    const data = await input.client.proveTransact(sppProofInputs, keys.proving);
    const transact = await transactInstruction({
      payer: vault,
      ownerSigners: [vault],
      inputTree: input.client.tree,
      outputTree: input.client.tree,
      data,
    });
    return executeTransactionSyncInstruction({
      settings: input.settings,
      approvers: input.approvers,
      inner: [transact],
    });
  } finally {
    keys.shielded.destroy();
    keys.proving.destroy();
  }
}
