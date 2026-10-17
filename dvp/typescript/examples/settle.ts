import assert from "node:assert/strict";
import { createZolanaClient } from "@heliuslabs/zolana";
import { atSlot, LocalKeys, NullifierKeyProofAuthority } from "@heliuslabs/zolana/client";
import { transactInstruction } from "@heliuslabs/zolana/interface";
import {
  bytes32,
  initializePoseidon,
  randomBlinding,
  poseidon,
  ShieldedKeypair,
  SigningKey,
  ViewingKey,
  P256_PUBLIC_KEY_LENGTH,
} from "@heliuslabs/zolana/keypair";
import {
  AssetRegistry,
  ConfidentialTransfer,
  createExternalData,
  decodeProgramTransaction,
  SppProofInputs,
  decodeConfidential,
  decodeOutputData,
  decryptToBalances,
  ProgramOwner,
  ProofInputUtxo,
  SOL_MINT,
  Utxo,
  toProgramWalletUtxo,
  type IndexedShieldedTransaction,
} from "@heliuslabs/zolana/transaction";
import {
  address,
  appendTransactionMessageInstructions,
  assertIsTransactionWithBlockhashLifetime,
  createTransactionMessage,
  getProgramDerivedAddress,
  getSignatureFromTransaction,
  pipe,
  sendTransactionWithoutConfirmingFactory,
  setTransactionMessageConfig,
  setTransactionMessageFeePayerSigner,
  setTransactionMessageLifetimeUsingBlockhash,
  signTransactionMessageWithSigners,
  type Instruction,
} from "@solana/kit";
import {
  circuitTerms,
  decodeTerms,
  dvpId,
  encodeTerms,
  encoder,
  PROGRAM_ID,
  type SwapDvp,
} from "../src/terms.js";
import { createDvpInstruction, settleDvpInstruction } from "../src/instructions.js";
import { provingKey, wasm, type WalletUtxo } from "../src/wasm.js";

async function main(): Promise<void> {
  // 1. Connect the seller, buyer, and settlement authority.
  await initializePoseidon();
  const seller = keypair("DVP_SELLER_SEED");
  const buyer = keypair("DVP_BUYER_SEED");
  const settlementAuthority = keypair("DVP_AUTHORITY_SEED");
  const client = await createZolanaClient({
    solanaRpcUrl: env("ZOLANA_LOCALNET_URL"),
    indexerUrl: env("ZOLANA_INDEXER_URL"),
    proverUrl: env("ZOLANA_PROVER_URL"),
    allowInsecureHttp: true,
    proofDataSource: "client",
  });
  const payer = seller.toSolanaSigner();
  const deliveryMint = address(env("DVP_DELIVERY_MINT"));
  const assets = new AssetRegistry([[BigInt(env("DVP_DELIVERY_ASSET_ID")), deliveryMint]]);
  const sendAndConfirm = sendAndConfirmFactory(client, payer);
  const pdaKey = ProgramOwner.nullifierKey();
  const pdaKeys = new NullifierKeyProofAuthority(pdaKey, client.proofService);
  const authorityViewingKey = settlementAuthority.viewingKey();
  try {
    // 2. Agree on the DvP terms and derive its authority PDA.
    const terms: SwapDvp = {
      seller: seller.toSolanaSigner().address,
      buyer: buyer.toSolanaSigner().address,
      creator: payer.address,
      settlementAuthority: settlementAuthority.toSolanaSigner().address,
      deliveryMint,
      paymentMint: SOL_MINT,
      deliveryAmount: 100_000n,
      paymentAmount: 250_000n,
      expiryTimestamp: BigInt(Math.floor(Date.now() / 1000) + 600),
      nonce: BigInt(Date.now()),
      refString: new Uint8Array(64),
      blinding: randomBlinding(),
      sellerSettlementDestination: seller.shieldedAddress(),
      buyerSettlementDestination: buyer.shieldedAddress(),
      sellerRefundDestination: seller.shieldedAddress(),
      buyerRefundDestination: buyer.shieldedAddress(),
    };
    const id = dvpId(terms);
    const [pda, bump] = await getProgramDerivedAddress({
      programAddress: PROGRAM_ID,
      seeds: [new TextEncoder().encode("dvp"), encoder.encode(terms.creator), id],
    });
    const dvpAuthority = new ProgramOwner(pda);
    const dvpAddress = dvpAuthority.address(settlementAuthority.shieldedAddress().viewingPublicKey);
    const checkedTerms = circuitTerms(terms, assets);

    // 3. Build and prove the create transaction with the program's generated client.
    const create = wasm.createDvpTransaction(
      {
        private: {
          txContext: {
            blindingSeed: poseidon([new TextEncoder().encode("DVPC"), terms.blinding]),
            outputTreeId: client.treeId,
          },
          terms: checkedTerms,
        },
        public: {
          creatorIdentity: checkedTerms.creatorIdentity,
          dvpId: id,
          dvpOwner: dvpAddress.toBytes(),
        },
      },
      seller.shieldedAddress().toBytes(),
      payer.address,
    );
    const createKey = provingKey("create_dvp");
    const createProver = wasm.CreateDvpProver.fromKey(createKey.key, createKey.checksum);
    let createProof;
    try {
      createProof = createProver.prove(create.proofInputs);
      assert.deepEqual(createProof.publicInputs, [create.publicHash]);
    } finally {
      createProver.free();
    }
    const created = await decodeProgramTransaction(create.finalizedTx, assets);

    // 4. Encrypt the terms for all three parties and prove the SPP transaction.
    const termsKey = ViewingKey.generate();
    let createSpp;
    try {
      const encrypted = created.encrypt(termsKey);
      const externalData = createExternalData({
        txViewingPublicKey: encrypted.txViewingPublicKey,
        salt: encrypted.salt,
        outputs: encrypted.outputs.map(({ utxoHash, ownerTag }) => ({ utxoHash, ownerTag })),
        resolvedOwnerTags: created.resolvedOwnerTags.slice(0, encrypted.outputs.length),
        messages: [seller, buyer, settlementAuthority].map((party, index) => ({
          viewTag: party.shieldedAddress().confidentialViewTag(),
          data: termsKey.encryptSlot(
            party.shieldedAddress().viewingPublicKey,
            encodeTerms(terms),
            encrypted.salt,
            1 + index,
          ),
        })),
      });
      createSpp = await client.proveTransact(
        new SppProofInputs({
          payer: payer.address,
          inputUtxos: created.inputs,
          outputs: created.outputs,
          externalData,
          blindingSeed: created.blindingSeed,
          outputTreeId: client.treeId,
        }),
        pdaKeys,
      );
    } finally {
      termsKey.destroy();
    }
    assert.deepEqual(createSpp.privateTxHash, created.paddingIndependentPrivateTxHash);
    const createTx = await sendAndConfirm(
      await createDvpInstruction({
        payer,
        signer: payer,
        tree: client.tree,
        authority: dvpAuthority,
        bump,
        dvpId: id,
        proof: createProof.compressedProof,
        transact: createSpp,
      }),
    );

    // 5. The buyer fetches the confirmed transaction and decrypts the terms.
    const createResponse = await client.getShieldedTransactionsBySignature(
      createTx.signature,
      atSlot(createTx.slot),
    );
    const createRecord = createResponse.transactions.at(0)?.transaction;
    assert.ok(createRecord?.txViewingPublicKey && createRecord.salt);
    const buyerMessageIndex = createRecord.messages.findIndex((message) =>
      equal(message.viewTag, buyer.shieldedAddress().confidentialViewTag()),
    );
    const buyerMessage = createRecord.messages.at(buyerMessageIndex);
    assert.ok(buyerMessageIndex >= 0 && buyerMessage);
    const buyerViewingKey = buyer.viewingKey();
    try {
      const discovered = decodeTerms(
        buyerViewingKey.decryptUtxo(
          buyerMessage.data,
          createRecord.txViewingPublicKey,
          createRecord.salt,
          1 + buyerMessageIndex,
        ),
      );
      assert.deepEqual(encodeTerms(discovered), encodeTerms(terms));
    } finally {
      buyerViewingKey.destroy();
    }
    const stateHash = created.outputHashes.at(0);
    assert.ok(stateHash);
    const stateSlot = createRecord.outputSlots.find((output) =>
      equal(output.outputContext.hash, stateHash),
    );
    const stateOutput = create.finalizedTx.outputUtxos.at(0);
    assert.ok(stateSlot && stateOutput);
    const dvpUtxo: WalletUtxo = {
      utxo: {
        owner: dvpAuthority.publicKey().toBytes(),
        asset: stateOutput.asset,
        amount: stateOutput.amount,
        blinding: stateOutput.blinding,
      },
      nullifierPubkey: ProgramOwner.nullifierPublicKey(),
      utxoHash: stateSlot.outputContext.hash,
      nullifier: pdaKey.nullifier(stateSlot.outputContext.hash, bytes32(stateOutput.blinding)),
      dataHash: id,
      treeId: client.treeId,
      leafIndex: stateSlot.outputContext.leafIndex,
    };

    // 6. Select the delivery funds from the seller's private balance.
    const deliveryHistory: IndexedShieldedTransaction[] = [];
    let deliveryCursor: Uint8Array | undefined;
    do {
      const page = await client.getShieldedTransactionsByTags(
        {
          tags: [seller.shieldedAddress().confidentialViewTag(), seller.viewingPublicKey().x()],
          limit: 50,
          ...(deliveryCursor === undefined ? {} : { cursor: deliveryCursor }),
        },
        atSlot(createTx.slot),
      );
      deliveryHistory.push(...page.transactions);
      deliveryCursor = page.nextCursor;
    } while (deliveryCursor !== undefined);
    const deliveryBalances = await decryptToBalances({
      keypair: seller,
      registry: assets,
      transactions: deliveryHistory,
    });
    const deliverySource = deliveryBalances
      .balance(deliveryMint)
      .utxos.find((utxo) => utxo.amount >= terms.deliveryAmount + 100n);
    assert.ok(deliverySource, "Fund the seller's private delivery balance first");
    const deliveryInput = ProofInputUtxo.fromKeypair(deliverySource, seller, {}, client.treeId);

    // 7. Build, encrypt, and prove the delivery transfer to the DvP PDA.
    const deliveryTransfer = new ConfidentialTransfer(
      seller.shieldedAddress(),
      [deliveryInput],
      seller.toSolanaSigner().address,
    ).withOutputTreeId(client.treeId);
    deliveryTransfer.send(dvpAddress, deliveryMint, terms.deliveryAmount + 100n);
    const deliveryProofInputs = deliveryTransfer.sign(seller, assets);
    const deliveryKeys = LocalKeys.fromKeypair(seller, client.proofService);
    let deliverySpp;
    try {
      deliverySpp = await client.proveTransact(deliveryProofInputs, deliveryKeys);
    } finally {
      deliveryKeys.destroy();
    }
    const deliveryTx = await sendAndConfirm(
      await transactInstruction({
        payer: seller.toSolanaSigner(),
        inputTree: client.tree,
        outputTree: client.tree,
        data: deliverySpp,
      }),
    );

    // 8. Fetch and decrypt the delivery deposit as the settlement authority.
    const deliveryResponse = await client.getShieldedTransactionsBySignature(
      deliveryTx.signature,
      atSlot(deliveryTx.slot),
    );
    const deliveryRecord = deliveryResponse.transactions.at(0)?.transaction;
    assert.ok(deliveryRecord?.txViewingPublicKey && deliveryRecord.salt);
    const deliveryIndex = deliveryRecord.outputSlots.findIndex((output) =>
      equal(output.viewTag, dvpAuthority.ownerTag()),
    );
    const deliverySlot = deliveryRecord.outputSlots.at(deliveryIndex);
    assert.ok(deliveryIndex >= 0 && deliverySlot);
    const deliveryPlaintext = decodeConfidential(
      authorityViewingKey.decryptUtxo(
        decodeOutputData(deliverySlot.payload).body.subarray(P256_PUBLIC_KEY_LENGTH),
        deliveryRecord.txViewingPublicKey,
        deliveryRecord.salt,
        deliveryIndex,
      ),
    );
    assert.equal(assets.resolve(deliveryPlaintext.assetId), deliveryMint);
    const deliveryDeposit = toProgramWalletUtxo(
      {
        utxo: new Utxo({
          owner: dvpAuthority.publicKey(),
          asset: deliveryMint,
          amount: deliveryPlaintext.amount,
          blinding: deliveryPlaintext.blinding,
          data: deliveryPlaintext.data,
        }),
        outputContext: deliverySlot.outputContext,
        nullifier: pdaKey.nullifier(deliverySlot.outputContext.hash, deliveryPlaintext.blinding),
        spent: false,
      },
      { nullifierPublicKey: ProgramOwner.nullifierPublicKey(), treeId: client.treeId, assets },
    );

    // 9. Select the payment funds from the buyer's private balance.
    const paymentHistory: IndexedShieldedTransaction[] = [];
    let paymentCursor: Uint8Array | undefined;
    do {
      const page = await client.getShieldedTransactionsByTags(
        {
          tags: [buyer.shieldedAddress().confidentialViewTag(), buyer.viewingPublicKey().x()],
          limit: 50,
          ...(paymentCursor === undefined ? {} : { cursor: paymentCursor }),
        },
        atSlot(createTx.slot),
      );
      paymentHistory.push(...page.transactions);
      paymentCursor = page.nextCursor;
    } while (paymentCursor !== undefined);
    const paymentBalances = await decryptToBalances({
      keypair: buyer,
      registry: assets,
      transactions: paymentHistory,
    });
    const paymentSource = paymentBalances
      .balance(SOL_MINT)
      .utxos.find((utxo) => utxo.amount >= terms.paymentAmount);
    assert.ok(paymentSource, "Fund the buyer's private payment balance first");
    const paymentInput = ProofInputUtxo.fromKeypair(paymentSource, buyer, {}, client.treeId);

    // 10. Build, encrypt, and prove the payment transfer to the DvP PDA.
    const paymentTransfer = new ConfidentialTransfer(
      buyer.shieldedAddress(),
      [paymentInput],
      buyer.toSolanaSigner().address,
    ).withOutputTreeId(client.treeId);
    paymentTransfer.send(dvpAddress, SOL_MINT, terms.paymentAmount);
    const paymentProofInputs = paymentTransfer.sign(buyer, assets);
    const paymentKeys = LocalKeys.fromKeypair(buyer, client.proofService);
    let paymentSpp;
    try {
      paymentSpp = await client.proveTransact(paymentProofInputs, paymentKeys);
    } finally {
      paymentKeys.destroy();
    }
    const paymentTx = await sendAndConfirm(
      await transactInstruction({
        payer: buyer.toSolanaSigner(),
        inputTree: client.tree,
        outputTree: client.tree,
        data: paymentSpp,
      }),
    );

    // 11. Fetch and decrypt the payment deposit as the settlement authority.
    const paymentResponse = await client.getShieldedTransactionsBySignature(
      paymentTx.signature,
      atSlot(paymentTx.slot),
    );
    const paymentRecord = paymentResponse.transactions.at(0)?.transaction;
    assert.ok(paymentRecord?.txViewingPublicKey && paymentRecord.salt);
    const paymentIndex = paymentRecord.outputSlots.findIndex((output) =>
      equal(output.viewTag, dvpAuthority.ownerTag()),
    );
    const paymentSlot = paymentRecord.outputSlots.at(paymentIndex);
    assert.ok(paymentIndex >= 0 && paymentSlot);
    const paymentPlaintext = decodeConfidential(
      authorityViewingKey.decryptUtxo(
        decodeOutputData(paymentSlot.payload).body.subarray(P256_PUBLIC_KEY_LENGTH),
        paymentRecord.txViewingPublicKey,
        paymentRecord.salt,
        paymentIndex,
      ),
    );
    assert.equal(assets.resolve(paymentPlaintext.assetId), SOL_MINT);
    const paymentDeposit = toProgramWalletUtxo(
      {
        utxo: new Utxo({
          owner: dvpAuthority.publicKey(),
          asset: SOL_MINT,
          amount: paymentPlaintext.amount,
          blinding: paymentPlaintext.blinding,
          data: paymentPlaintext.data,
        }),
        outputContext: paymentSlot.outputContext,
        nullifier: pdaKey.nullifier(paymentSlot.outputContext.hash, paymentPlaintext.blinding),
        spent: false,
      },
      { nullifierPublicKey: ProgramOwner.nullifierPublicKey(), treeId: client.treeId, assets },
    );

    // 12. Build and prove the settlement with the agreed deposits and current cluster time.
    const slot = await client.solanaRpc.getSlot().send();
    const now = await client.solanaRpc.getBlockTime(slot).send();
    assert.ok(now !== null, "Cluster time is unavailable");
    const currentTimestamp = now - 2n;
    const settlement = wasm.settleDvpTransaction(
      {
        private: {
          txContext: {
            blindingSeed: poseidon([new TextEncoder().encode("DVPS"), terms.blinding]),
            outputTreeId: client.treeId,
          },
          terms: checkedTerms,
          dvpUtxo,
          deliveryDeposit,
          paymentDeposit,
        },
        public: {
          creatorIdentity: checkedTerms.creatorIdentity,
          dvpId: id,
          dvpOwner: dvpAddress.toBytes(),
          settlementAuthorityIdentity: checkedTerms.settlementAuthorityIdentity,
          currentTimestamp,
        },
      },
      settlementAuthority.shieldedAddress().toBytes(),
      payer.address,
    );
    const settlementKey = provingKey("settle_dvp");
    const settlementProver = wasm.SettleDvpProver.fromKey(
      settlementKey.key,
      settlementKey.checksum,
    );
    let settlementProof;
    try {
      settlementProof = settlementProver.prove(settlement.proofInputs);
      assert.deepEqual(settlementProof.publicInputs, [settlement.publicHash]);
    } finally {
      settlementProver.free();
    }

    // 13. Encrypt, prove, and submit the atomic SPP settlement.
    const settled = await decodeProgramTransaction(settlement.finalizedTx, assets);
    const settlementInputs = settled.toProofInputs(settlementAuthority);
    const settlementSpp = await client.proveTransact(settlementInputs, pdaKeys);
    assert.deepEqual(settlementSpp.privateTxHash, settled.paddingIndependentPrivateTxHash);
    const settlementTx = await sendAndConfirm(
      await settleDvpInstruction({
        payer,
        signer: settlementAuthority.toSolanaSigner(),
        tree: client.tree,
        authority: dvpAuthority,
        creator: terms.creator,
        bump,
        dvpId: id,
        currentTimestamp,
        proof: settlementProof.compressedProof,
        transact: settlementSpp,
      }),
    );

    // 14. Fetch the confirmed settlement and check both payments and surplus refunds.
    const response = await client.getShieldedTransactionsBySignature(
      settlementTx.signature,
      atSlot(settlementTx.slot),
    );
    const transactions = response.transactions.map(({ transaction }) => transaction);
    const sellerBalances = await decryptToBalances({
      keypair: seller,
      registry: assets,
      transactions,
    });
    const buyerBalances = await decryptToBalances({
      keypair: buyer,
      registry: assets,
      transactions,
    });
    assert.equal(buyerBalances.balance(deliveryMint).amount, terms.deliveryAmount);
    assert.equal(sellerBalances.balance(SOL_MINT).amount, terms.paymentAmount);
    assert.equal(sellerBalances.balance(deliveryMint).amount, 100n);
    assert.equal(buyerBalances.balance(SOL_MINT).amount, 0n);
    console.log(`settle: ${settlementTx.signature}`);
  } finally {
    authorityViewingKey.destroy();
    pdaKeys.destroy();
    pdaKey.destroy();
    seller.destroy();
    buyer.destroy();
    settlementAuthority.destroy();
  }
}

function env(name: string): string {
  const value = process.env[name];
  if (!value) throw new Error(`Set ${name}; see README.md`);
  return value;
}

function keypair(name: string): ShieldedKeypair {
  const seed = bytes32(Uint8Array.from(Buffer.from(env(name), "hex")));
  try {
    return ShieldedKeypair.fromKeypair(SigningKey.fromEd25519Bytes(seed));
  } finally {
    seed.fill(0);
  }
}

function equal(left: Uint8Array, right: Uint8Array): boolean {
  return Buffer.from(left).equals(Buffer.from(right));
}

function sendAndConfirmFactory(
  client: Awaited<ReturnType<typeof createZolanaClient>>,
  payer: ReturnType<ShieldedKeypair["toSolanaSigner"]>,
) {
  const send = sendTransactionWithoutConfirmingFactory({ rpc: client.solanaRpc });
  return async (instruction: Instruction) => {
    const { value: lifetime } = await client.solanaRpc.getLatestBlockhash().send();
    const signed = await signTransactionMessageWithSigners(
      pipe(
        createTransactionMessage({ version: 1 }),
        (message) => setTransactionMessageFeePayerSigner(payer, message),
        (message) => setTransactionMessageLifetimeUsingBlockhash(lifetime, message),
        (message) =>
          setTransactionMessageConfig(
            { computeUnitLimit: 1_400_000, loadedAccountsDataSizeLimit: 64 * 1024 * 1024 },
            message,
          ),
        (message) => appendTransactionMessageInstructions([instruction], message),
      ),
    );
    assertIsTransactionWithBlockhashLifetime(signed);
    const signature = getSignatureFromTransaction(signed);
    await send(signed, { commitment: "confirmed" });
    const slot = await client.confirmTransaction(signature);
    return { signature, slot };
  };
}

await main();
