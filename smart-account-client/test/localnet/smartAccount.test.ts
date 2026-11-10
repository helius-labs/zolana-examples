/**
 * Create, Register, Spend and Rotate against the release localnet
 * (scripts/localnet.sh). One test, because each step builds on the last.
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
import { ShieldedKeypair, initializePoseidon } from "@heliuslabs/spr/keypair";
import { DepositAsset, depositInstruction } from "@heliuslabs/spr/interface";
import { getRegisterInstructionAsync } from "@heliuslabs/spr/instructions";
import { LocalShieldedKeys } from "@heliuslabs/spr/transaction";

import { recoverSharedSecrets } from "../../src/keyRecord.js";
import {
  checkOwnEntry,
  createSmartAccount,
  fetchRegisteredViewingKey,
  fetchSettings,
  memberFromKeypair,
  recoverVaultKeys,
  registerVaultInstruction,
  rotateSmartAccount,
  spendInstruction,
  vaultKeys,
  type Client,
  type Member,
} from "../../src/smartAccount.js";
import { Permission } from "../../src/squads.js";
import { InMemoryKeyRecordStore, currentKeyRecord } from "../../src/store.js";
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

async function depositToVault(
  client: Client,
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
    const store = new InMemoryKeyRecordStore();
    const [alice, bob, carol] = [0, 1, 2].map(() =>
      memberFromKeypair(ShieldedKeypair.generate()),
    );
    assert(alice && bob && carol);
    const recipient = ShieldedKeypair.generate();
    for (const member of [alice, bob, carol]) {
      await airdrop(client, member.signer.address, 20n * SOL);
      // 1. Derive: each signer registers its own keys; rotate reads kept
      // signers' viewing keys from the registry.
      await send(client, member.signer, [
        await getRegisterInstructionAsync({
          owner: member.signer,
          nullifierPublicKey: member.keypair.nullifierPublicKey(),
          viewingPublicKey: member.keypair.viewingPublicKey().toBytes(),
        }),
      ]);
    }

    // 1. Create: Alice creates a 2-of-2 account for Alice and Bob, uploads
    // the key record, then sends the create.
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
    await store.put(created.record);
    // The vault pays the registry rent and the SPP fees inside the Squads CPIs.
    await send(client, alice.signer, [
      created.instruction,
      transferSol(alice, created.vault, 2n * SOL),
    ]);
    const settings = await fetchSettings(client, created.settings);
    assert.equal(settings.threshold, 2);
    assert.deepEqual(
      settings.signers.map((signer) => signer.key).sort(),
      [alice.signer.address, bob.signer.address].sort(),
    );

    // Check: each signer decrypts its entry of the current record.
    const bobKeys = await recoverVaultKeys({
      client,
      store,
      settings: created.settings,
      member: bob,
    });
    assert.deepEqual(bobKeys.record, created.record);

    // Register: Alice and Bob register the vault's shared keys.
    await send(client, alice.signer, [
      await registerVaultInstruction({
        client,
        settings: created.settings,
        approvers: [alice.signer, bob.signer],
        secrets: bobKeys.secrets,
      }),
    ]);
    assert.deepEqual(
      await fetchRegisteredViewingKey(client, created.vault),
      created.record.sharedViewingPublicKey,
    );

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
      store,
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
    // Approve: Alice stays a signer, so she checks her entry before signing.
    checkOwnEntry(rotation.record, alice);
    await store.put(rotation.record);
    await send(client, alice.signer, rotation.instructions);
    const rotated = await fetchSettings(client, created.settings);
    assert.deepEqual(
      rotated.signers.map((signer) => signer.key).sort(),
      [alice.signer.address, carol.signer.address].sort(),
    );
    assert.deepEqual(
      await fetchRegisteredViewingKey(client, created.vault),
      rotation.record.sharedViewingPublicKey,
    );
    const current = await currentKeyRecord(store, {
      settings: created.settings,
      signers: rotated.signers.map((signer) => signer.key),
      registeredViewingPublicKey: rotation.record.sharedViewingPublicKey,
    });
    assert.equal(current.epoch, 1n);
    assert.deepEqual(
      current.nullifierPublicKey,
      created.record.nullifierPublicKey,
    );
    assert.throws(
      () => recoverSharedSecrets(current, bob.signer.address, bob.keypair),
      /no entry/,
    );

    // Alice was a signer before the rotate, so she still opens the vault's
    // older UTXO with the earlier viewing key, and spends it with Carol.
    const aliceKeys = await recoverVaultKeys({
      client,
      store,
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
      store,
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
    // carries only the registry key update.
    const keyOnly = await rotateSmartAccount({
      client,
      store,
      settings: created.settings,
      rotator: carol,
      approvers: [alice.signer, carol.signer],
      keep: [alice.signer.address, carol.signer.address],
      add: [],
      threshold: 2,
    });
    assert.equal(keyOnly.instructions.length, 1);
    checkOwnEntry(keyOnly.record, alice);
    await store.put(keyOnly.record);
    await send(client, carol.signer, keyOnly.instructions);
    assert.deepEqual(
      await fetchRegisteredViewingKey(client, created.vault),
      keyOnly.record.sharedViewingPublicKey,
    );
    const carolAfter = await recoverVaultKeys({
      client,
      store,
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
