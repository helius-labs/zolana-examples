/**
 * Create, Register, Spend and Rotate against the release localnet with the
 * branch user registry (scripts/localnet.sh). The main test is one test
 * because each step builds on the last.
 */
import assert from "node:assert/strict";
import { test } from "node:test";

import {
  AccountRole,
  address,
  type Address,
  type Instruction,
} from "@solana/kit";
import { SOL_MINT } from "@heliuslabs/spr";
import type { SprClient } from "@heliuslabs/spr/client";
import { ShieldedKeypair, initializePoseidon } from "@heliuslabs/spr/keypair";
import { DepositAsset, depositInstruction } from "@heliuslabs/spr/interface";
import { getRegisterInstructionAsync } from "@heliuslabs/spr/instructions";
import { LocalShieldedKeys } from "@heliuslabs/spr/transaction";

import { recoverSharedSecrets } from "../../src/keyRecord.js";
import { fetchVaultRegistry } from "../../src/registry.js";
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
import { Permission } from "../../src/squads.js";
import { airdrop, localClient, send, syncedSolBalance } from "./send.js";

const SOL = 1_000_000_000n;
const SYSTEM_PROGRAM = address("11111111111111111111111111111111");

function transferSol(from: Member, to: Address, amount: bigint): Instruction {
  const data = new Uint8Array(12);
  data[0] = 2;
  new DataView(data.buffer).setBigUint64(4, amount, true);
  return {
    programAddress: SYSTEM_PROGRAM,
    accounts: [
      {
        address: from.signer.address,
        role: AccountRole.WRITABLE_SIGNER,
        signer: from.signer,
      },
      { address: to, role: AccountRole.WRITABLE },
    ],
    data,
  } as Instruction;
}

/**
 * Funds `count` fresh members. Each registers its own keys, because rotate
 * reads kept signers' viewing keys from the registry.
 */
async function registeredMembers(
  client: SprClient,
  count: number,
): Promise<Member[]> {
  const members = Array.from({ length: count }, () =>
    memberFromKeypair(ShieldedKeypair.generate()),
  );
  for (const member of members) {
    await airdrop(client, member.signer.address, 20n * SOL);
    await send(client, member.signer, [
      await getRegisterInstructionAsync({
        owner: member.signer,
        nullifierPublicKey: member.keypair.nullifierPublicKey(),
        viewingPublicKey: member.keypair.viewingPublicKey().toBytes(),
      }),
    ]);
  }
  return members;
}

/** Alice creates a 2-of-2 account for Alice and Bob and funds its vault. */
async function createTwoOfTwo(client: SprClient, alice: Member, bob: Member) {
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
  await send(client, alice.signer, [
    created.instruction,
    transferSol(alice, created.vault, 2n * SOL),
  ]);
  return created;
}

/** The registered viewing key and the current key record of `vault`. */
async function vaultRegistry(client: SprClient, vault: Address) {
  const registry = await fetchVaultRegistry(client, vault);
  assert(registry !== undefined, "the vault is registered");
  return registry;
}

async function depositToVault(
  client: SprClient,
  depositor: Member,
  vaultAddress: ReturnType<typeof vaultKeys>["address"],
  amount: bigint,
): Promise<void> {
  await send(client, depositor.signer, [
    await depositInstruction({
      tree: client.tree,
      depositor: depositor.signer,
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
  async () => {
    await initializePoseidon();
    const client = await localClient();
    // 1. Derive: each signer registers its own keys.
    const [alice, bob, carol] = await registeredMembers(client, 3);
    assert(alice && bob && carol);
    const recipient = ShieldedKeypair.generate();

    // 1. Create: Alice creates a 2-of-2 account for Alice and Bob.
    const created = await createTwoOfTwo(client, alice, bob);
    const settings = await fetchSettings(client, created.settings);
    assert.equal(settings.threshold, 2);
    assert.deepEqual(
      settings.signers.map((signer) => signer.key).sort(),
      [alice.signer.address, bob.signer.address].sort(),
    );

    // Check, then Register: each approver opens its entry of record 0, and
    // the vault registers the keys and sets the record as its registry data.
    checkOwnEntry(created.record, alice);
    checkOwnEntry(created.record, bob);
    await send(client, alice.signer, [
      await registerVaultInstruction({
        client,
        settings: created.settings,
        approvers: [alice.signer, bob.signer],
        record: created.record,
      }),
    ]);
    assert.deepEqual(await vaultRegistry(client, created.vault), {
      viewingPublicKey: created.record.sharedViewingPublicKey,
      record: created.record,
    });
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
    await depositToVault(client, carol, vault.address, 10_000_000n);
    const funded = await syncedSolBalance(
      client,
      vault.address,
      vault.shielded,
      10_000_000n,
    );

    // 3. Spend: Bob pays 3,000,000 lamports to the recipient, Alice approves.
    await send(client, bob.signer, [
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
    checkOwnEntry(rotation.record, alice);
    await send(client, alice.signer, rotation.instructions);
    const rotated = await fetchSettings(client, created.settings);
    assert.deepEqual(
      rotated.signers.map((signer) => signer.key).sort(),
      [alice.signer.address, carol.signer.address].sort(),
    );
    assert.deepEqual(await vaultRegistry(client, created.vault), {
      viewingPublicKey: rotation.record.sharedViewingPublicKey,
      record: rotation.record,
    });
    assert.deepEqual(
      rotation.record.nullifierPublicKey,
      created.record.nullifierPublicKey,
    );
    assert.throws(
      () =>
        recoverSharedSecrets(rotation.record, bob.signer.address, bob.keypair),
      /no entry/,
    );

    // Alice was a signer before the rotate, so she still opens the vault's
    // older UTXO with the earlier viewing key, which she reads from record 0
    // in the registry record's history, and spends it with Carol.
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
    await send(client, carol.signer, [
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

    // Carol joined after the first deposit: she opens only UTXOs sent to the
    // new shared viewing key, here the change of Alice's spend.
    const carolKeys = await recoverVaultKeys({
      client,
      settings: created.settings,
      member: carol,
    });
    assert.equal(carolKeys.viewingKeys.length, 1);
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
      approvers: [alice.signer, carol.signer],
      keep: [alice.signer.address, carol.signer.address],
      add: [],
      threshold: 2,
    });
    assert.equal(keyOnly.instructions.length, 1);
    checkOwnEntry(keyOnly.record, alice);
    await send(client, carol.signer, keyOnly.instructions);
    assert.deepEqual(await vaultRegistry(client, created.vault), {
      viewingPublicKey: keyOnly.record.sharedViewingPublicKey,
      record: keyOnly.record,
    });
    const carolAfter = await recoverVaultKeys({
      client,
      settings: created.settings,
      member: carol,
    });
    assert.equal(carolAfter.record.epoch, 2n);
    assert.equal(carolAfter.viewingKeys.length, 2);
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
  async () => {
    await initializePoseidon();
    const client = await localClient();
    const [alice, bob] = await registeredMembers(client, 2);
    assert(alice && bob);
    const created = await createTwoOfTwo(client, alice, bob);
    const rotateKeys = {
      client,
      settings: created.settings,
      rotator: alice,
      approvers: [alice.signer, bob.signer],
      keep: [alice.signer.address, bob.signer.address],
      add: [],
      threshold: 2,
    };

    // No key record is on chain before Register.
    await assert.rejects(rotateSmartAccount(rotateKeys), /not registered/);

    const register = await registerVaultInstruction({
      client,
      settings: created.settings,
      approvers: [alice.signer, bob.signer],
      record: created.record,
    });
    await send(client, alice.signer, [register]);

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
    await send(client, alice.signer, rotation.instructions);
    await assert.rejects(
      send(client, alice.signer, [register]),
      (error: Error) =>
        /requires an uninitialized account/.test(String(error.cause)),
    );
    assert.deepEqual(await vaultRegistry(client, created.vault), {
      viewingPublicKey: rotation.record.sharedViewingPublicKey,
      record: rotation.record,
    });
  },
);
