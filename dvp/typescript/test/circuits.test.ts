import { test } from "node:test";
import assert from "node:assert/strict";
import { address, getProgramDerivedAddress } from "@solana/kit";
import {
  initializePoseidon,
  ShieldedAddress,
  ShieldedPublicKey,
  NullifierKey,
  ViewingKey,
  ShieldedKeypair,
  SigningKey,
  randomBlinding,
  bytes31,
} from "@heliuslabs/zolana/keypair";
import {
  AssetRegistry,
  ProgramOwner,
  ProofInputUtxo,
  Utxo,
  SOL_MINT,
  decodeProgramTransaction,
} from "@heliuslabs/zolana/transaction";
import {
  circuitTerms,
  encodeTerms,
  decodeTerms,
  dvpId,
  field,
  h,
  integer,
  bytes32,
  decoder,
  encoder,
  PROGRAM_ID,
  type SwapDvp,
} from "../src/terms.js";
import { ProgramProver, wasm, type WalletUtxo } from "../src/wasm.js";

await initializePoseidon();

test("terms match the X25519 Rust commitment and exact wire encoding", () => {
  const destination = (seed: number) => {
    const nullifier = NullifierKey.fromSecret(bytes31(new Uint8Array(31).fill(seed))),
      viewing = ViewingKey.fromBytes(bytes32(new Uint8Array(32).fill(seed)));
    try {
      return ShieldedAddress.fromPublicKeys(
        ShieldedPublicKey.fromEd25519(bytes32(new Uint8Array(32).fill(seed))),
        nullifier.publicKey(),
        viewing.publicKey(),
      );
    } finally {
      nullifier.destroy();
      viewing.destroy();
    }
  };
  const pk = (seed: number) => decoder.decode(new Uint8Array(32).fill(seed));
  const refString = new Uint8Array(64);
  refString.set(new TextEncoder().encode("dvp vector"));
  const t: SwapDvp = {
    seller: pk(1),
    buyer: pk(2),
    deliveryMint: pk(3),
    paymentMint: SOL_MINT,
    settlementAuthority: pk(4),
    creator: pk(5),
    deliveryAmount: 1000n,
    paymentAmount: 2500000000n,
    expiryTimestamp: 1800000000n,
    earliestSettlementTimestamp: 1700000000n,
    nonce: 7n,
    refString,
    blinding: bytes32(Uint8Array.from({ length: 32 }, (_, i) => i)),
    sellerSettlementDestination: destination(11),
    buyerSettlementDestination: destination(12),
    sellerRefundDestination: destination(13),
    buyerRefundDestination: destination(14),
  };
  assert.equal(
    integer(dvpId(t)).toString(),
    "13601006363444597634864084490330890110481277484464045593977180451704799918864",
  );
  const encoded = encodeTerms(t);
  assert.equal(encoded.length, 725);
  assert.deepEqual(encodeTerms(decodeTerms(encoded)), encoded);
  assert.throws(() => decodeTerms(encoded.slice(1)));
  assert.throws(() => decodeTerms(new Uint8Array([...encoded, 0])));
});
test("Node proves each generated program circuit and every abort funding combination", async () => {
  // 1. Create the DvP with a compact output and the pinned program key.
  const f = await fixture();
  try {
    const create = wasm.createDvpTransaction(
      {
        private: {
          txContext: { blindingSeed: h(field(0x44565043n), f.terms.blinding), outputTreeId: 0 },
          terms: f.checkedTerms,
        },
        public: {
          creatorIdentity: f.checkedTerms.creatorIdentity,
          dvpId: integer(f.id).toString(),
          dvpOwner: f.dvpAddress.toBytes(),
        },
      },
      f.seller.shieldedAddress().toBytes(),
      f.terms.creator,
    );
    const createProver = new ProgramProver("create_dvp");
    try {
      const firstProof = createProver.prove(create);
      const secondProof = createProver.prove(create);
      assert.deepEqual(firstProof.publicInputs, [create.publicHash]);
      assert.deepEqual(secondProof.publicInputs, firstProof.publicInputs);
      assert.notDeepEqual(secondProof.compressedProof, firstProof.compressedProof);
      assert.throws(
        () =>
          createProver.prove({
            proofInputs: create.proofInputs,
            publicHash: create.publicHash.map((byte) => byte ^ 1),
          }),
        /proof public hash differs from its transaction/,
      );
    } finally {
      createProver.free();
    }
    const created = await decodeProgramTransaction(create.finalizedTx, f.assets);
    assert.equal(created.inputs.at(0)?.isAddress(), true);
    assert.equal(created.outputs.at(1)?.isCompact(), true);
    const createInputs = created.encryptWithKeys(f.seller);
    assert.deepEqual(createInputs.privateTxHash(), created.paddingIndependentPrivateTxHash);
    assert.equal(createInputs.inputUtxoHashes().length, 0);
    assert.equal(createInputs.externalData.outputs.length, 1);
    const stateOutput = create.finalizedTx.outputUtxos.at(0);
    const stateHash = create.finalizedTx.outputHashes.at(0);
    assert.ok(stateOutput && stateHash);
    const dvpUtxo: WalletUtxo = {
      utxo: {
        owner: f.authority.publicKey().toBytes(),
        asset: stateOutput.asset,
        amount: 0n,
        blinding: stateOutput.blinding,
      },
      nullifierPubkey: ProgramOwner.nullifierPublicKey(),
      utxoHash: stateHash,
      nullifier: f.pdaKey.nullifier(bytes32(stateHash), bytes32(stateOutput.blinding)),
      dataHash: f.id,
      treeId: 0,
      leafIndex: 9n,
    };

    // 2. Settle both legs and retain both real surplus refunds.
    const settlement = wasm.settleDvpTransaction(
      {
        private: {
          txContext: { blindingSeed: h(field(0x44565053n), f.terms.blinding), outputTreeId: 0 },
          terms: f.checkedTerms,
          dvpUtxo,
          deliveryDeposit: f.delivery,
          paymentDeposit: f.payment,
        },
        public: {
          creatorIdentity: f.checkedTerms.creatorIdentity,
          dvpId: integer(f.id).toString(),
          dvpOwner: f.dvpAddress.toBytes(),
          settlementAuthorityIdentity: f.checkedTerms.settlementAuthorityIdentity,
          currentTimestamp: 1750000000n,
        },
      },
      f.settlementAuthority.shieldedAddress().toBytes(),
      f.terms.creator,
    );
    const settlementProver = new ProgramProver("settle_dvp");
    try {
      assert.deepEqual(settlementProver.prove(settlement).publicInputs, [settlement.publicHash]);
    } finally {
      settlementProver.free();
    }
    const settled = await decodeProgramTransaction(settlement.finalizedTx, f.assets);
    assert.deepEqual(
      settled.outputs.map((output) => output.amount),
      [1000n, 2500n, 100n, 200n],
    );
    assert.ok(settled.outputs.every((output) => !output.isDummy()));
    assert.deepEqual(
      settled.encryptWithKeys(f.settlementAuthority).privateTxHash(),
      settled.paddingIndependentPrivateTxHash,
    );

    // 3. Cancel and reject with every combination of included deposits.
    const absent = nativeInput(ProofInputUtxo.dummy(randomBlinding(), 0), f.assets, 0n);
    const abortProver = new ProgramProver("abort_dvp");
    try {
      for (const reject of [false, true]) {
        for (const delivery of [absent, f.delivery]) {
          for (const payment of [absent, f.payment]) {
            const abort = wasm.abortDvpTransaction(
              {
                private: {
                  txContext: {
                    blindingSeed: h(field(0x44565041n), f.terms.blinding),
                    outputTreeId: 0,
                  },
                  terms: f.checkedTerms,
                  dvpUtxo,
                  deliveryDeposit: delivery,
                  paymentDeposit: payment,
                  buyerRejects: reject,
                },
                public: {
                  creatorIdentity: f.checkedTerms.creatorIdentity,
                  dvpId: integer(f.id).toString(),
                  dvpOwner: f.dvpAddress.toBytes(),
                  signerIdentity: reject
                    ? f.checkedTerms.buyerIdentity
                    : f.checkedTerms.settlementAuthorityIdentity,
                  reject,
                },
              },
              (reject ? f.buyer : f.settlementAuthority).shieldedAddress().toBytes(),
              f.terms.creator,
            );
            assert.deepEqual(abortProver.prove(abort).publicInputs, [abort.publicHash]);
            const aborted = await decodeProgramTransaction(abort.finalizedTx, f.assets);
            assert.deepEqual(
              aborted.outputs.map((output) => output.amount),
              [delivery.utxo.amount, payment.utxo.amount],
            );
            assert.ok(aborted.outputs.every((output) => !output.isDummy()));
            assert.equal(aborted.inputs.length, 3);
            assert.deepEqual(
              aborted.encryptWithKeys(reject ? f.buyer : f.settlementAuthority).privateTxHash(),
              aborted.paddingIndependentPrivateTxHash,
            );
          }
        }
      }
    } finally {
      abortProver.free();
    }

    // 4. Reclaim the delivery deposit with compact output padding.
    const reclaim = wasm.reclaimDvpTransaction(
      {
        private: {
          txContext: { blindingSeed: randomBlinding(), outputTreeId: 0 },
          terms: f.checkedTerms,
          deposit: f.delivery,
          paymentLeg: false,
          refundDestination: f.seller.shieldedAddress().toBytes(),
        },
        public: {
          creatorIdentity: f.checkedTerms.creatorIdentity,
          dvpId: integer(f.id).toString(),
          dvpOwner: f.dvpAddress.toBytes(),
          signerIdentity: f.checkedTerms.sellerIdentity,
        },
      },
      f.seller.shieldedAddress().toBytes(),
      f.terms.creator,
    );
    const reclaimProver = new ProgramProver("reclaim_dvp");
    try {
      assert.deepEqual(reclaimProver.prove(reclaim).publicInputs, [reclaim.publicHash]);
    } finally {
      reclaimProver.free();
    }
    const reclaimed = await decodeProgramTransaction(reclaim.finalizedTx, f.assets);
    assert.equal(reclaimed.outputs.at(0)?.amount, 1100n);
    assert.equal(reclaimed.outputs.at(1)?.isCompact(), true);
    assert.deepEqual(
      reclaimed.encryptWithKeys(f.seller).privateTxHash(),
      reclaimed.paddingIndependentPrivateTxHash,
    );

    // 5. Refuse a signer or seed that differs from the agreed terms.
    assert.throws(() =>
      wasm.createDvpTransaction(
        {
          private: {
            txContext: { blindingSeed: randomBlinding(), outputTreeId: 0 },
            terms: f.checkedTerms,
          },
          public: {
            creatorIdentity: f.checkedTerms.creatorIdentity,
            dvpId: integer(f.id).toString(),
            dvpOwner: f.dvpAddress.toBytes(),
          },
        },
        f.seller.shieldedAddress().toBytes(),
        f.terms.creator,
      ),
    );
    assert.throws(() =>
      wasm.settleDvpTransaction(
        {
          private: {
            txContext: { blindingSeed: h(field(0x44565053n), f.terms.blinding), outputTreeId: 0 },
            terms: f.checkedTerms,
            dvpUtxo,
            deliveryDeposit: f.delivery,
            paymentDeposit: f.payment,
          },
          public: {
            creatorIdentity: f.checkedTerms.creatorIdentity,
            dvpId: integer(f.id).toString(),
            dvpOwner: f.dvpAddress.toBytes(),
            settlementAuthorityIdentity: f.checkedTerms.sellerIdentity,
            currentTimestamp: 1750000000n,
          },
        },
        f.seller.shieldedAddress().toBytes(),
        f.terms.creator,
      ),
    );
  } finally {
    f.pdaKey.destroy();
    f.seller.destroy();
    f.buyer.destroy();
    f.settlementAuthority.destroy();
  }
});

async function fixture() {
  const keypair = (seed: number) =>
    ShieldedKeypair.fromKeypair(
      SigningKey.fromEd25519Bytes(bytes32(new Uint8Array(32).fill(seed))),
    );
  const seller = keypair(1);
  const buyer = keypair(2);
  const settlementAuthority = keypair(4);
  const deliveryMint = address("So11111111111111111111111111111111111111112");
  const assets = new AssetRegistry([[2n, deliveryMint]]);
  const terms: SwapDvp = {
    seller: seller.toSolanaSigner().address,
    buyer: buyer.toSolanaSigner().address,
    settlementAuthority: settlementAuthority.toSolanaSigner().address,
    creator: seller.toSolanaSigner().address,
    deliveryMint,
    paymentMint: SOL_MINT,
    deliveryAmount: 1000n,
    paymentAmount: 2500n,
    expiryTimestamp: 1800000000n,
    earliestSettlementTimestamp: 1700000000n,
    nonce: 7n,
    refString: new Uint8Array(64),
    blinding: bytes32(new Uint8Array(32).fill(7)),
    sellerSettlementDestination: seller.shieldedAddress(),
    buyerSettlementDestination: buyer.shieldedAddress(),
    sellerRefundDestination: seller.shieldedAddress(),
    buyerRefundDestination: buyer.shieldedAddress(),
  };
  const id = dvpId(terms);
  const [pda] = await getProgramDerivedAddress({
    programAddress: PROGRAM_ID,
    seeds: [new TextEncoder().encode("dvp"), encoder.encode(terms.creator), id],
  });
  const authority = new ProgramOwner(pda);
  const pdaKey = ProgramOwner.nullifierKey();
  const deposit = (asset: import("@solana/kit").Address, amount: bigint, leaf: bigint) =>
    nativeInput(
      ProofInputUtxo.fromNullifierKey(
        new Utxo({
          owner: authority.publicKey(),
          asset,
          amount,
          blinding: randomBlinding(),
        }),
        pdaKey,
        {},
        0,
      ),
      assets,
      leaf,
    );
  return {
    seller,
    buyer,
    settlementAuthority,
    terms,
    id,
    authority,
    pdaKey,
    assets,
    checkedTerms: circuitTerms(terms, assets),
    dvpAddress: authority.address(settlementAuthority.shieldedAddress().viewingPublicKey),
    delivery: deposit(deliveryMint, 1100n, 10n),
    payment: deposit(SOL_MINT, 2700n, 11n),
  };
}

function nativeInput(input: ProofInputUtxo, assets: AssetRegistry, leafIndex: bigint): WalletUtxo {
  return {
    utxo: {
      owner: input.utxo.owner.toBytes(),
      asset: { asset: input.utxo.asset, assetId: assets.assetId(input.utxo.asset) },
      amount: input.utxo.amount,
      blinding: input.utxo.blinding,
    },
    nullifierPubkey: input.nullifierPublicKey,
    utxoHash: input.hash(),
    nullifier: input.nullifier(),
    treeId: input.treeId,
    leafIndex,
    ...(input.dataHash === undefined ? {} : { dataHash: input.dataHash }),
  };
}
