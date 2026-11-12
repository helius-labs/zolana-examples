/**
 * Create, Register, Spend and Rotate on devnet or mainnet using the deployed
 * programs. The main test is one test
 * because each step builds on the last.
 */
import assert from "node:assert/strict";
import { test, type TestContext } from "node:test";

import { type Address, type TransactionSigner } from "@solana/kit";
import { SOL_MINT } from "@heliuslabs/spr";
import type { SprClient } from "@heliuslabs/spr/client";
import { ShieldedKeypair, initializePoseidon } from "@heliuslabs/spr/keypair";
import { DepositAsset, depositInstruction } from "@heliuslabs/spr/interface";
import { getRegisterInstructionAsync } from "@heliuslabs/spr/instructions";
import { LocalShieldedKeys } from "@heliuslabs/spr/transaction";

import { recoverSharedSecrets, type KeyRecord } from "../../src/keyRecord.js";
import {
  fetchVaultRegistry,
  prepareKeyRecordUpload,
  type VaultRegistry,
} from "../../src/registry.js";
import {
  EMPTY_RECORD_ADDRESS,
  fetchStorageHead,
  fetchStoredRecord,
  type RecordUpload,
} from "../../src/storage.js";
import {
  checkOwnEntry,
  createSmartAccount,
  fetchSettings,
  memberFromKeypair,
  recoverVaultKeys,
  registerVaultInstruction,
  rotateSmartAccount,
  spendInstruction,
  vaultKeys,
  type Member,
} from "../../src/smartAccount.js";
import {
  Permission,
  executeTransactionSyncInstruction,
} from "../../src/squads.js";
import { networkSetup, send, syncedSolBalance, transferSol } from "./send.js";

/** Partial uploads never change the active head; abandoned uploads are refunded. */
async function uploadRecord(
  client: SprClient,
  payer: TransactionSigner,
  owner: Address,
  upload: RecordUpload,
  t: TestContext,
): Promise<void> {
  t.after(async () => {
    if ((await client.getAccount(upload.address)) === undefined) return;
    if (!(await fetchStoredRecord(client, upload.address)).sealed) {
      await send(client, payer, [upload.closeInstruction]);
    }
  });
  const previous = await fetchStorageHead(client, owner);
  for (const instruction of upload.instructions) {
    await send(client, payer, [instruction]);
    assert.deepEqual(
      await fetchStorageHead(client, owner),
      previous,
      "upload leaves the active head unchanged",
    );
  }
}

/**
 * Registers `count` fresh members. Each registers its own keys, because rotate
 * reads kept signers' viewing keys from the registry.
 */
async function registeredMembers(
  client: SprClient,
  payer: TransactionSigner,
  count: number,
  t: TestContext,
): Promise<Member[]> {
  const members = Array.from({ length: count }, () =>
    memberFromKeypair(ShieldedKeypair.generate()),
  );
  t.after(async () => {
    for (const member of members) {
      const balance = await client.getBalance(member.signer.address);
      if (balance > 0n) {
        await send(client, payer, [
          transferSol(member.signer, payer.address, balance),
        ]);
      }
    }
  });
  for (const member of members) {
    await send(client, payer, [
      await getRegisterInstructionAsync({
        owner: member.signer,
        payer,
        nullifierPublicKey: member.keypair.nullifierPublicKey(),
        viewingPublicKey: member.keypair.viewingPublicKey().toBytes(),
      }),
    ]);
  }
  return members;
}

/** Alice creates a 2-of-2 account for Alice and Bob and funds its vault. */
async function createTwoOfTwo(
  client: SprClient,
  payer: TransactionSigner,
  members: readonly Member[],
  t: TestContext,
) {
  const [alice, bob] = members;
  assert(alice && bob);
  // The creator pays Squads' account rent; the sponsor pays transaction fees.
  await send(client, payer, [
    transferSol(payer, alice.signer.address, 30_000_000n),
  ]);
  const created = await createSmartAccount({
    client,
    creator: alice.signer,
    threshold: 2,
    signers: [alice, bob].map((member) => ({
      key: member.signer.address,
      permissions: Permission.All,
      viewingPublicKey: member.keypair.viewingPublicKey(),
    })),
  });
  // The vault pays the registry rent and the SPP fees inside the Squads CPIs.
  t.after(async () => {
    const balance = await client.getBalance(created.vault);
    if (balance === 0n) return;
    const settings = await fetchSettings(client, created.settings);
    const approvers = members
      .filter((member) =>
        settings.signers.some(({ key }) => key === member.signer.address),
      )
      .map((member) => member.signer);
    await send(client, payer, [
      await executeTransactionSyncInstruction({
        settings: created.settings,
        approvers,
        inner: [transferSol(created.vault, payer.address, balance)],
      }),
    ]);
  });
  await send(client, payer, [
    created.instruction,
    transferSol(payer, created.vault, 20_000_000n),
  ]);
  return created;
}

/** The registered viewing key and the current key record of `vault`. */
async function vaultRegistry(client: SprClient, vault: Address) {
  const registry = await fetchVaultRegistry(client, vault);
  assert(registry !== undefined, "the vault is registered");
  return registry;
}

/**
 * The registry state after `upload` of `record` became the vault's head; the
 * upload's payer is its default refund destination.
 */
function publishedRegistry(
  record: KeyRecord,
  upload: RecordUpload,
  revision: bigint,
  payer: TransactionSigner,
): VaultRegistry {
  return {
    viewingPublicKey: record.sharedViewingPublicKey,
    current: {
      address: upload.address,
      revision,
      refund: payer.address,
      record,
    },
  };
}

async function depositToVault(
  client: SprClient,
  depositor: TransactionSigner,
  vaultAddress: ReturnType<typeof vaultKeys>["address"],
  amount: bigint,
): Promise<void> {
  await send(client, depositor, [
    await depositInstruction({
      tree: client.tree,
      depositor,
      deposits: [
        {
          asset: DepositAsset.sol(),
          viewTag: vaultAddress.confidentialViewTag(),
          recipientOwnerHash: vaultAddress.ownerHash(),
          amount,
        },
      ],
    }),
  ]);
}

test(
  "create, register, spend and rotate a private smart account",
  { timeout: 600_000 },
  async (t) => {
    await initializePoseidon();
    const { client, payer } = await networkSetup();
    // 1. Derive: each signer registers its own keys.
    const members = await registeredMembers(client, payer, 3, t);
    const [alice, bob, carol] = members;
    assert(alice && bob && carol);
    const recipient = ShieldedKeypair.generate();

    // 1. Create: Alice creates a 2-of-2 account for Alice and Bob.
    const created = await createTwoOfTwo(client, payer, members, t);
    const settings = await fetchSettings(client, created.settings);
    assert.equal(settings.threshold, 2);
    assert.deepEqual(
      settings.signers.map((signer) => signer.key).sort(),
      [alice.signer.address, bob.signer.address].sort(),
    );

    // Check, then Register: each approver opens its entry of record 0, and
    // the vault registers the keys and sets the record as its registry data.
    for (const approver of [alice, bob]) {
      await checkOwnEntry({
        client,
        settings: created.settings,
        record: created.record,
        member: approver,
      });
    }
    const firstUpload = await prepareKeyRecordUpload({
      owner: created.vault,
      previous: EMPTY_RECORD_ADDRESS,
      payer,
      record: created.record,
    });
    await uploadRecord(client, payer, created.vault, firstUpload, t);
    await send(client, payer, [
      await registerVaultInstruction({
        client,
        settings: created.settings,
        approvers: [alice.signer, bob.signer],
        record: created.record,
        buffer: firstUpload.address,
      }),
    ]);
    assert.deepEqual(
      await vaultRegistry(client, created.vault),
      publishedRegistry(created.record, firstUpload, 1n, payer),
    );
    const bobKeys = await recoverVaultKeys({
      client,
      settings: created.settings,
      member: bob,
    });
    assert.deepEqual(bobKeys.record, created.record);

    // Fund the vault's private balance.
    const vault = vaultKeys(
      client,
      created.vault,
      bobKeys.secrets,
      bobKeys.viewingKeys,
    );
    await depositToVault(client, payer, vault.address, 10_000_000n);
    const funded = await syncedSolBalance(
      client,
      vault.address,
      vault.shielded,
      10_000_000n,
    );

    // 3. Spend: Bob initiates a 3,000,000-lamport payment, Alice approves.
    await send(client, payer, [
      await spendInstruction({
        client,
        settings: created.settings,
        approvers: [alice.signer, bob.signer],
        secrets: bobKeys.secrets,
        viewingKeys: bobKeys.viewingKeys,
        inputs: funded.balance(SOL_MINT).utxos,
        payments: [
          {
            recipient: recipient.shieldedAddress(),
            asset: SOL_MINT,
            amount: 3_000_000n,
          },
        ],
      }),
    ]);
    await syncedSolBalance(client, vault.address, vault.shielded, 7_000_000n);
    await syncedSolBalance(
      client,
      recipient.shieldedAddress(),
      LocalShieldedKeys.fromKeypair(recipient),
      3_000_000n,
    );

    // 2. Rotate: Alice replaces Bob with Carol, threshold stays 2.
    const rotation = await rotateSmartAccount({
      client,
      settings: created.settings,
      rotator: alice,
      payer,
      approvers: [alice.signer, bob.signer],
      keep: [alice.signer.address],
      add: [
        {
          key: carol.signer.address,
          permissions: Permission.All,
          viewingPublicKey: carol.keypair.viewingPublicKey(),
        },
      ],
      threshold: 2,
    });
    assert.equal(rotation.record.epoch, 1n);
    // Approve: Alice stays a signer, so she checks her entry before signing.
    await checkOwnEntry({
      client,
      settings: created.settings,
      record: rotation.record,
      member: alice,
    });
    await uploadRecord(client, payer, created.vault, rotation.upload, t);
    await send(client, payer, rotation.instructions);
    const rotated = await fetchSettings(client, created.settings);
    assert.deepEqual(
      rotated.signers.map((signer) => signer.key).sort(),
      [alice.signer.address, carol.signer.address].sort(),
    );
    assert.deepEqual(
      await vaultRegistry(client, created.vault),
      publishedRegistry(rotation.record, rotation.upload, 2n, payer),
    );
    assert.deepEqual(
      rotation.record.nullifierPublicKey,
      created.record.nullifierPublicKey,
    );
    assert.throws(
      () =>
        recoverSharedSecrets(rotation.record, bob.signer.address, bob.keypair),
      /no entry/,
    );

    // Carol can decrypt the remaining UTXO even though it predates her membership.
    const carolKeys = await recoverVaultKeys({
      client,
      settings: created.settings,
      member: carol,
    });
    assert.equal(carolKeys.viewingKeys.length, 2);
    const carolVault = vaultKeys(
      client,
      created.vault,
      carolKeys.secrets,
      carolKeys.viewingKeys,
    );
    await syncedSolBalance(
      client,
      carolVault.address,
      carolVault.shielded,
      7_000_000n,
    );

    // Alice was a signer before the rotate, so she still opens the vault's
    // older UTXO with the earlier viewing key retained in the current record,
    // and spends it with Carol.
    const aliceKeys = await recoverVaultKeys({
      client,
      settings: created.settings,
      member: alice,
    });
    assert.equal(aliceKeys.viewingKeys.length, 2);
    const aliceVault = vaultKeys(
      client,
      created.vault,
      aliceKeys.secrets,
      aliceKeys.viewingKeys,
    );
    const beforeSpend = await syncedSolBalance(
      client,
      aliceVault.address,
      aliceVault.shielded,
      7_000_000n,
    );
    await send(client, payer, [
      await spendInstruction({
        client,
        settings: created.settings,
        approvers: [alice.signer, carol.signer],
        secrets: aliceKeys.secrets,
        viewingKeys: aliceKeys.viewingKeys,
        inputs: beforeSpend.balance(SOL_MINT).utxos,
        payments: [
          {
            recipient: recipient.shieldedAddress(),
            asset: SOL_MINT,
            amount: 2_000_000n,
          },
        ],
      }),
    ]);

    // The same keys also open the change sent under the new viewing key.
    await syncedSolBalance(
      client,
      carolVault.address,
      carolVault.shielded,
      5_000_000n,
    );
    await syncedSolBalance(
      client,
      recipient.shieldedAddress(),
      LocalShieldedKeys.fromKeypair(recipient),
      5_000_000n,
    );

    // A key-only rotate: same signers and threshold, so the transaction
    // carries only the registry key update and the new record.
    const keyOnly = await rotateSmartAccount({
      client,
      settings: created.settings,
      rotator: carol,
      payer,
      approvers: [alice.signer, carol.signer],
      keep: [alice.signer.address, carol.signer.address],
      add: [],
      threshold: 2,
    });
    assert.equal(keyOnly.instructions.length, 1);
    await checkOwnEntry({
      client,
      settings: created.settings,
      record: keyOnly.record,
      member: alice,
    });
    await uploadRecord(client, payer, created.vault, keyOnly.upload, t);
    await send(client, payer, keyOnly.instructions);
    assert.deepEqual(
      await vaultRegistry(client, created.vault),
      publishedRegistry(keyOnly.record, keyOnly.upload, 3n, payer),
    );
    assert.equal(await client.getAccount(firstUpload.address), undefined);
    assert.equal(await client.getAccount(rotation.upload.address), undefined);
    const carolAfter = await recoverVaultKeys({
      client,
      settings: created.settings,
      member: carol,
    });
    assert.equal(carolAfter.record.epoch, 2n);
    assert.equal(carolAfter.viewingKeys.length, 3);
    const carolAfterVault = vaultKeys(
      client,
      created.vault,
      carolAfter.secrets,
      carolAfter.viewingKeys,
    );
    await syncedSolBalance(
      client,
      carolAfterVault.address,
      carolAfterVault.shielded,
      5_000_000n,
    );
  },
);

test(
  "rotate needs a registered vault, and a second register fails on chain",
  { timeout: 300_000 },
  async (t) => {
    await initializePoseidon();
    const { client, payer } = await networkSetup();
    const members = await registeredMembers(client, payer, 2, t);
    const [alice, bob] = members;
    assert(alice && bob);
    const created = await createTwoOfTwo(client, payer, members, t);
    const rotateKeys = {
      client,
      settings: created.settings,
      rotator: alice,
      payer,
      approvers: [alice.signer, bob.signer],
      keep: [alice.signer.address, bob.signer.address],
      add: [],
      threshold: 2,
    };

    // No key record is on chain before Register.
    await assert.rejects(rotateSmartAccount(rotateKeys), /not registered/);

    const firstUpload = await prepareKeyRecordUpload({
      owner: created.vault,
      previous: EMPTY_RECORD_ADDRESS,
      payer,
      record: created.record,
    });
    await uploadRecord(client, payer, created.vault, firstUpload, t);
    const register = await registerVaultInstruction({
      client,
      settings: created.settings,
      approvers: [alice.signer, bob.signer],
      record: created.record,
      buffer: firstUpload.address,
    });
    await send(client, payer, [register]);

    // Adding a current signer is refused; it belongs in `keep`.
    await assert.rejects(
      rotateSmartAccount({
        ...rotateKeys,
        keep: [alice.signer.address],
        add: [
          {
            key: bob.signer.address,
            permissions: Permission.Vote,
            viewingPublicKey: bob.keypair.viewingPublicKey(),
          },
        ],
      }),
      /already a signer/,
    );

    // A rotate, then a replay of the register: the registry refuses to
    // create the vault's record twice, so the replay cannot put record 0's
    // keys back.
    const rotation = await rotateSmartAccount(rotateKeys);
    await uploadRecord(client, payer, created.vault, rotation.upload, t);
    await send(client, payer, rotation.instructions);
    await assert.rejects(send(client, payer, [register]), (error: Error) =>
      /requires an uninitialized account/.test(String(error.cause)),
    );
    assert.deepEqual(
      await vaultRegistry(client, created.vault),
      publishedRegistry(rotation.record, rotation.upload, 2n, payer),
    );
  },
);
