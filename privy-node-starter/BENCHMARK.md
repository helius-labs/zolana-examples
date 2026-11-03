# Privy Confidential Transfers -- Devnet Benchmark

Measurements from the Privy Node starter's Express endpoints on Solana Devnet. Compute units and transaction sizes come from confirmed transaction metadata. HTTP duration includes wallet setup, synchronization, proving when required, Privy signing, and confirmation.

Regenerate with `npm run confidential:e2e`.

## Definitions

- **Total CU**: Compute units consumed by the complete transaction.
- **HTTP Duration**: Elapsed time from the E2E request until the confirmed endpoint response.
- **Fee**: Transaction fee in lamports; account rent is separate.
- **Instruction Data**: Combined instruction data, including compute-budget and account-setup instructions.
- **v0 Tx**: Serialized transaction size, including signatures.
- **Proving Time**: Not measured separately; included in HTTP duration for transfers and withdrawals.

## Table of Contents

1. [Deposit](#1-deposit)
2. [Transfer](#2-transfer)
3. [Withdrawal](#3-withdrawal)

## 1. Deposit

| Asset | Total CU | HTTP Duration | Fee            |
| ----- | -------- | ------------- | -------------- |
| SOL   | 38,080   | 2,645.9 ms    | 5,000 lamports |

**Transaction Size**

| Instruction Data | Accounts | v0 Tx     |
| ---------------- | -------- | --------- |
| 83 bytes         | 6        | 390 bytes |

Transaction: [Devnet SOL deposit](https://explorer.solana.com/tx/3f3rTf2HfPzQwYUFf9L7hgYjrGe7SJ6WzoigsjfGuainmLWLXG9LRwZgh5oLVaTsoRmcX3bioHgQDg38oKrbjX5R?cluster=devnet).

## 2. Transfer

No completed transfer measurement from this run. The prover rejected the SDK's attestation route before proof generation.

## 3. Withdrawal

No withdrawal measurement from this run. The transfer stage did not complete.

**Run Conditions**

- Date: 2026-11-03.
- Runtime: Node.js 24.21.0.
- Privy Node SDK: 0.1.0-alpha.2.
- Privacy SDK: @heliuslabs/zolana 0.7.0-alpha.
- Network: Solana Devnet.
- Two Privy Solana wallets with a shared owner authorization key.
- Wallet registrations and SPL mint setup confirmed before the SOL deposit.
- The attempted transfer returned CLIENT_PROVER_TEE_ATTESTATION. The SDK requests /tee/v1/attestation with the Zolana-Tee wire contract; the deployed prover serves /attestation with the Spr-Tee wire contract.
- Attestation verification remains enabled. This report contains only the successful deposit measurement.
