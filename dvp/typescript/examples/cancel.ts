import assert from "node:assert/strict";
import { createZolanaClient } from "@heliuslabs/zolana";
import { atSlot, NullifierKeyProofAuthority } from "@heliuslabs/zolana/client";
import {
  bytes32,
  initializePoseidon,
  randomBlinding,
  poseidon,
  ShieldedKeypair,
  SigningKey,
  ViewingKey,
} from "@heliuslabs/zolana/keypair";
import {
  AssetRegistry,
  createExternalData,
  decodeProgramTransaction,
  SppProofInputs,
  decryptToBalances,
  ProgramOwner,
  ProofInputUtxo,
  SOL_MINT,
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
import { createDvpInstruction, cancelDvpInstruction } from "../src/instructions.js";
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

    // 6. Include padding for each absent deposit.
    const padding = ProofInputUtxo.dummy(randomBlinding(), client.treeId);
    const absentDeposit: WalletUtxo = {
      utxo: {
        owner: padding.utxo.owner.toBytes(),
        asset: { asset: SOL_MINT, assetId: 1n },
        amount: 0n,
        blinding: padding.utxo.blinding,
      },
      nullifierPubkey: padding.nullifierPublicKey,
      utxoHash: padding.hash(),
      nullifier: padding.nullifier(),
      treeId: client.treeId,
      leafIndex: 0n,
    };

    // 7. Build and prove cancellation by the settlement authority.
    const abort = wasm.abortDvpTransaction(
      {
        private: {
          txContext: {
            blindingSeed: poseidon([new TextEncoder().encode("DVPA"), terms.blinding]),
            outputTreeId: client.treeId,
          },
          terms: checkedTerms,
          dvpUtxo,
          deliveryDeposit: absentDeposit,
          paymentDeposit: absentDeposit,
          buyerRejects: false,
        },
        public: {
          creatorIdentity: checkedTerms.creatorIdentity,
          dvpId: id,
          dvpOwner: dvpAddress.toBytes(),
          signerIdentity: checkedTerms.settlementAuthorityIdentity,
          reject: false,
        },
      },
      settlementAuthority.shieldedAddress().toBytes(),
      payer.address,
    );
    const abortKey = provingKey("abort_dvp");
    const abortProver = wasm.AbortDvpProver.fromKey(abortKey.key, abortKey.checksum);
    let abortProof;
    try {
      abortProof = abortProver.prove(abort.proofInputs);
      assert.deepEqual(abortProof.publicInputs, [abort.publicHash]);
    } finally {
      abortProver.free();
    }

    // 8. Encrypt, prove, and submit both refunds.
    const aborted = await decodeProgramTransaction(abort.finalizedTx, assets);
    const abortInputs = aborted.toProofInputs(settlementAuthority);
    const abortSpp = await client.proveTransact(abortInputs, pdaKeys);
    assert.deepEqual(abortSpp.privateTxHash, aborted.paddingIndependentPrivateTxHash);
    const abortTx = await sendAndConfirm(
      await cancelDvpInstruction({
        payer,
        signer: settlementAuthority.toSolanaSigner(),
        tree: client.tree,
        authority: dvpAuthority,
        creator: terms.creator,
        bump,
        dvpId: id,
        proof: abortProof.compressedProof,
        transact: abortSpp,
      }),
    );

    // 9. Fetch both real refunds and check the DvP was consumed.
    const response = await client.getShieldedTransactionsBySignature(
      abortTx.signature,
      atSlot(abortTx.slot),
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
    assert.equal(sellerBalances.balance(deliveryMint).amount, 0n);
    assert.equal(buyerBalances.balance(SOL_MINT).amount, 0n);
    assert.equal(aborted.outputs.length, 2);
    assert.ok(aborted.outputs.every((output) => !output.isDummy()));
    assert.ok(
      transactions.some((transaction) =>
        transaction.nullifiers.some((nullifier) => equal(nullifier, dvpUtxo.nullifier)),
      ),
    );
    console.log(`cancel: ${abortTx.signature}`);
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
