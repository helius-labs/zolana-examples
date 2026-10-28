import assert from "node:assert/strict";
import { test } from "node:test";
import {
  bytes32,
  bytes33,
  initializePoseidon,
  X25519PublicKey,
  ShieldedKeypair,
  SigningKey,
} from "@heliuslabs/zolana/keypair";
import {
  AssetRegistry,
  ConfidentialTransfer,
  decodeConfidential,
  decodeOutputData,
  ProgramOwner,
  ProofInputUtxo,
  SOL_MINT,
  Utxo,
  toProgramWalletUtxo,
} from "@heliuslabs/zolana/transaction";
import { getTreeAddress } from "@heliuslabs/zolana/addresses";
import { PROGRAM_ID, VIEWING_PUBLIC_KEY_LENGTH } from "../src/terms.js";

test("the seller recovers its deposit using its transaction viewing key", async () => {
  // 1. Create the seller, settlement authority, and DvP owner.
  await initializePoseidon();
  const sellerSigningKey = SigningKey.fromEd25519Bytes(bytes32(new Uint8Array(32).fill(1)));
  const authoritySigningKey = SigningKey.fromEd25519Bytes(bytes32(new Uint8Array(32).fill(4)));
  const seller = ShieldedKeypair.fromKeypair(sellerSigningKey);
  const settlementAuthority = ShieldedKeypair.fromKeypair(authoritySigningKey);
  const owner = await ProgramOwner.find(["sender-refund"], PROGRAM_ID);
  const pdaKey = ProgramOwner.nullifierKey();
  const assets = new AssetRegistry();
  try {
    // 2. Encrypt a delivery deposit to the settlement authority.
    const source = ProofInputUtxo.fromKeypair(
      new Utxo({
        owner: seller.signingPublicKey(),
        asset: SOL_MINT,
        amount: 1_100n,
        blinding: bytes32(new Uint8Array(32).fill(7)),
      }),
      seller,
    );
    const transfer = new ConfidentialTransfer(
      seller.shieldedAddress(),
      [source],
      seller.toSolanaSigner().address,
    );
    transfer.send(owner.address(settlementAuthority.viewingPublicKey()), SOL_MINT, 1_000n);
    const inputs = transfer.sign(seller, assets);
    const outputIndex = inputs.externalData.outputs.findIndex((output) =>
      output.ownerTag.kind === "inline"
        ? Buffer.from(output.ownerTag.value).equals(Buffer.from(owner.ownerTag()))
        : false,
    );
    const published = inputs.externalData.outputs.at(outputIndex);
    assert.ok(outputIndex >= 0 && published?.data);

    // 3. Decrypt the same deposit as its sender.
    const senderTxKey = seller.transactionViewingKey(inputs.firstNullifier());
    try {
      assert.ok(senderTxKey.publicKey().equals(inputs.externalData.txViewingPublicKey));
      const encrypted = decodeOutputData(published.data).body;
      const recipientKey = X25519PublicKey.fromBytes(
        bytes33(encrypted.subarray(0, VIEWING_PUBLIC_KEY_LENGTH)),
      );
      const plaintext = decodeConfidential(
        senderTxKey.decryptSlotEphemeral(
          recipientKey,
          encrypted.subarray(VIEWING_PUBLIC_KEY_LENGTH),
          inputs.externalData.salt,
          outputIndex,
        ),
      );
      assert.equal(plaintext.amount, 1_000n);
      assert.equal(assets.resolve(plaintext.assetId), SOL_MINT);

      // 4. Check that the recovered fields reconstruct the indexed commitment.
      const recovered = toProgramWalletUtxo(
        {
          utxo: new Utxo({
            owner: owner.publicKey(),
            asset: SOL_MINT,
            amount: plaintext.amount,
            blinding: plaintext.blinding,
            data: plaintext.data,
          }),
          outputContext: { hash: published.utxoHash, tree: getTreeAddress(0), leafIndex: 9n },
          nullifier: pdaKey.nullifier(published.utxoHash, plaintext.blinding),
          spent: false,
        },
        { nullifierPublicKey: ProgramOwner.nullifierPublicKey(), treeId: 0, assets },
      );
      assert.deepEqual(recovered.utxoHash, published.utxoHash);
    } finally {
      senderTxKey.destroy();
    }
  } finally {
    pdaKey.destroy();
    seller.destroy();
    settlementAuthority.destroy();
  }
});
