# Privy Confidential Transfers -- Devnet Benchmark

Measurements from a complete Privy Node starter E2E run on Solana Devnet. All 18 transactions confirmed without onchain errors. Both wallets finished with zero private SOL and SPL balances. Compute units and transaction sizes come from confirmed RPC metadata.

Regenerate with `pnpm confidential:e2e`.

## Definitions

- **Total CU**: Compute units consumed by the complete transaction, including account setup and child instructions.
- **HTTP Duration**: Elapsed time from the E2E request until the confirmed endpoint response; includes wallet setup, synchronization, proving when required, Privy signing, and confirmation.
- **Fee**: Solana transaction fee in lamports. Account rent and protocol reimbursements are separate.
- **Instruction Data**: Combined instruction data, including compute-budget and account-setup instructions.
- **v0 Tx**: Serialized transaction size, including signatures.
- **Proving Time**: Included in HTTP duration; not measured separately.
- **Samples**: A single E2E run. Values are observations, not latency percentiles.

## Table of Contents

1. [Deposit](#1-deposit)
2. [Transfer](#2-transfer)
3. [Withdrawal](#3-withdrawal)
4. [Transactions](#4-transactions)

## 1. Deposit

| Asset | Case                              | Samples | Total CU | HTTP Duration      | Fee            |
| ----- | --------------------------------- | ------- | -------- | ------------------ | -------------- |
| SOL   | Base                              | 1       | 38,072   | 2,625.9 ms         | 5,000 lamports |
| SOL   | Repeated deposits                 | 2       | 38,076   | 2,225.2–2,573.9 ms | 5,000 lamports |
| SPL   | First deposit; interface creation | 1       | 45,738   | 2,792.3 ms         | 5,000 lamports |
| SPL   | Repeated deposits                 | 2       | 39,953   | 2,214.8–2,788.9 ms | 5,000 lamports |

**Transaction Size**

| Asset | Case                              | Instruction Data | Accounts | v0 Tx     |
| ----- | --------------------------------- | ---------------- | -------- | --------- |
| SOL   | Base                              | 83 bytes         | 6        | 390 bytes |
| SOL   | Repeated deposits                 | 83 bytes         | 6        | 390 bytes |
| SPL   | First deposit; interface creation | 85 bytes         | 12       | 597 bytes |
| SPL   | Repeated deposits                 | 84 bytes         | 8        | 457 bytes |

The first SPL deposit creates the privacy interface in the deposit transaction. Later deposits reuse it.

## 2. Transfer

| Asset | Private Inputs | Total CU | HTTP Duration | Fee            |
| ----- | -------------- | -------- | ------------- | -------------- |
| SOL   | 1              | 137,975  | 3,965.2 ms    | 5,000 lamports |
| SPL   | 1              | 143,058  | 3,924.5 ms    | 5,000 lamports |
| SOL   | 2              | 146,280  | 2,924.8 ms    | 5,000 lamports |
| SPL   | 2              | 148,237  | 3,370.6 ms    | 5,000 lamports |

**Transaction Size**

| Asset | Private Inputs | Instruction Data | Accounts | v0 Tx     |
| ----- | -------------- | ---------------- | -------- | --------- |
| SOL   | 1              | 619 bytes        | 6        | 929 bytes |
| SPL   | 1              | 619 bytes        | 6        | 929 bytes |
| SOL   | 2              | 652 bytes        | 7        | 995 bytes |
| SPL   | 2              | 652 bytes        | 7        | 995 bytes |

The two-input case transfers more than either individual deposited note contains. Recipients are resolved through their public registry records.

## 3. Withdrawal

| Asset | Case        | Wallet    | Total CU | HTTP Duration | Fee            |
| ----- | ----------- | --------- | -------- | ------------- | -------------- |
| SOL   | base        | Sender    | 142,639  | 3,611.8 ms    | 5,000 lamports |
| SOL   | base        | Recipient | 139,935  | 3,836.2 ms    | 5,000 lamports |
| SPL   | base        | Sender    | 148,451  | 4,141.2 ms    | 5,000 lamports |
| SPL   | base        | Recipient | 159,023  | 4,011.5 ms    | 5,000 lamports |
| SOL   | multi-input | Sender    | 141,534  | 3,835.9 ms    | 5,000 lamports |
| SOL   | multi-input | Recipient | 142,051  | 4,078.4 ms    | 5,000 lamports |
| SPL   | multi-input | Sender    | 146,839  | 4,208.8 ms    | 5,000 lamports |
| SPL   | multi-input | Recipient | 150,825  | 3,649.7 ms    | 5,000 lamports |

**Transaction Size**

| Asset | Instruction Data | Accounts | v0 Tx       |
| ----- | ---------------- | -------- | ----------- |
| SOL   | 597 bytes        | 7        | 941 bytes   |
| SPL   | 599 bytes        | 12       | 1,115 bytes |

The first recipient SPL withdrawal creates its associated token account. Each withdrawal runs after an Express restart and reconstructs the private balance from the indexer.

## 4. Transactions

| #   | Asset | Case        | Operation            | Transaction                                                                                                                                                                                                                        |
| --- | ----- | ----------- | -------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 1   | SOL   | base        | deposit              | [4FmWVHvqVubSLCyrtGtAYz5t31tikUSmF1D29yGworzj3qs2xpwR4irawYGxi2dGfWMTJrow7EqxBi8SK327vER3](https://explorer.solana.com/tx/4FmWVHvqVubSLCyrtGtAYz5t31tikUSmF1D29yGworzj3qs2xpwR4irawYGxi2dGfWMTJrow7EqxBi8SK327vER3?cluster=devnet) |
| 2   | SOL   | base        | transfer             | [2WjRCy3RnpYYQ4EP5cPZPW7EcSd6zDAo4PGLxar2XA57y2KLLNjoA4JuejK7b2ZnDrJ2VK3cnAdSAWScRFe6PC1P](https://explorer.solana.com/tx/2WjRCy3RnpYYQ4EP5cPZPW7EcSd6zDAo4PGLxar2XA57y2KLLNjoA4JuejK7b2ZnDrJ2VK3cnAdSAWScRFe6PC1P?cluster=devnet) |
| 3   | SOL   | base        | withdraw (sender)    | [4Bm5zciCE8o3U8EDABCiVoQvBMK3mJqXjfxFzERkGvCAAMaZx7PXSwVpLGdAtVphWfpEvSbKXgu5W7PBJXaqCvQ3](https://explorer.solana.com/tx/4Bm5zciCE8o3U8EDABCiVoQvBMK3mJqXjfxFzERkGvCAAMaZx7PXSwVpLGdAtVphWfpEvSbKXgu5W7PBJXaqCvQ3?cluster=devnet) |
| 4   | SOL   | base        | withdraw (recipient) | [3THdxAMcns9KvWRLW8QAvBdXK5f3EiyDJe8j2sA3WsTgdvmtmXErfGj4NWVvL4Ymr6nea5YWLxnUU5LPaR4JWRjr](https://explorer.solana.com/tx/3THdxAMcns9KvWRLW8QAvBdXK5f3EiyDJe8j2sA3WsTgdvmtmXErfGj4NWVvL4Ymr6nea5YWLxnUU5LPaR4JWRjr?cluster=devnet) |
| 5   | SPL   | base        | deposit              | [5UxmDt4PNxU2w4VjB1vRc6VQbRTqvZV7a8xAmcLhFV8YJdkzot1wwqTBCJdtSstw182QSDLpHHiBJ7grXsZHQ71S](https://explorer.solana.com/tx/5UxmDt4PNxU2w4VjB1vRc6VQbRTqvZV7a8xAmcLhFV8YJdkzot1wwqTBCJdtSstw182QSDLpHHiBJ7grXsZHQ71S?cluster=devnet) |
| 6   | SPL   | base        | transfer             | [SApD8PB1KYKtZfLz39Tx9fUFaCfuX9jKVMpJBUdKMAcVovx6hpGiUT6mLLHyxxS9xfcVH6DdG2VJTsrip95Jz98](https://explorer.solana.com/tx/SApD8PB1KYKtZfLz39Tx9fUFaCfuX9jKVMpJBUdKMAcVovx6hpGiUT6mLLHyxxS9xfcVH6DdG2VJTsrip95Jz98?cluster=devnet)   |
| 7   | SPL   | base        | withdraw (sender)    | [4KSkGTq6D27GKvVYa4DMQHsaFgbNnagzWms8G9xQQgGUa72Bbrt88iN13bndSmhm6Mjas2k1RAhpdeVJCD3LkVic](https://explorer.solana.com/tx/4KSkGTq6D27GKvVYa4DMQHsaFgbNnagzWms8G9xQQgGUa72Bbrt88iN13bndSmhm6Mjas2k1RAhpdeVJCD3LkVic?cluster=devnet) |
| 8   | SPL   | base        | withdraw (recipient) | [WcRQFdgxmMJiuB5AGPneztxZCPX4LgZ3ReqJqnm4Rypy5trf4CZH3fReDRxAgmnZXWy7k4ffy92asvphSG53dN1](https://explorer.solana.com/tx/WcRQFdgxmMJiuB5AGPneztxZCPX4LgZ3ReqJqnm4Rypy5trf4CZH3fReDRxAgmnZXWy7k4ffy92asvphSG53dN1?cluster=devnet)   |
| 9   | SOL   | multi-input | deposit              | [3TZDgLqiaPDmVPSAtFbRRgvupKi29QRRqHcxbyXn1Lm9CCH6JupVoDfB1hMyJhH8LaE4QhydUZ18sFDxuWh7wUd1](https://explorer.solana.com/tx/3TZDgLqiaPDmVPSAtFbRRgvupKi29QRRqHcxbyXn1Lm9CCH6JupVoDfB1hMyJhH8LaE4QhydUZ18sFDxuWh7wUd1?cluster=devnet) |
| 10  | SOL   | multi-input | deposit              | [X2ty1oTLCqv9wetGe1MLRM3kL8gxgjeWsk3x1oHLvHkFYaGPmaM2d1EHB9PizwfgSyJLWp5WvSKoKD6P8ANzDHf](https://explorer.solana.com/tx/X2ty1oTLCqv9wetGe1MLRM3kL8gxgjeWsk3x1oHLvHkFYaGPmaM2d1EHB9PizwfgSyJLWp5WvSKoKD6P8ANzDHf?cluster=devnet)   |
| 11  | SOL   | multi-input | transfer             | [PB6jw9Jt8C3rnyKKpZETbChsruJ1r5Dy9SrMRk9uWpXqw5KBFqMMx6gmYnXJWvPi8Q4nPdoQqGfr21Lijnr4k6x](https://explorer.solana.com/tx/PB6jw9Jt8C3rnyKKpZETbChsruJ1r5Dy9SrMRk9uWpXqw5KBFqMMx6gmYnXJWvPi8Q4nPdoQqGfr21Lijnr4k6x?cluster=devnet)   |
| 12  | SOL   | multi-input | withdraw (sender)    | [2DUWo7GhyUveaJUYA831f72oGY5BfwyTaPPZWb1EjaFkKBySWK48jWr7YcVKuTa2cQ5AUTQ7Tha29PimoDECtgWm](https://explorer.solana.com/tx/2DUWo7GhyUveaJUYA831f72oGY5BfwyTaPPZWb1EjaFkKBySWK48jWr7YcVKuTa2cQ5AUTQ7Tha29PimoDECtgWm?cluster=devnet) |
| 13  | SOL   | multi-input | withdraw (recipient) | [3ZCZ3DHtV9HtuasCX4rCccsNn25AN5QMmQ7hnXy7gyjJ8v7UC7HymvoAteaSnEBbCHh1NQz5BunpkSrq8rS8AxmN](https://explorer.solana.com/tx/3ZCZ3DHtV9HtuasCX4rCccsNn25AN5QMmQ7hnXy7gyjJ8v7UC7HymvoAteaSnEBbCHh1NQz5BunpkSrq8rS8AxmN?cluster=devnet) |
| 14  | SPL   | multi-input | deposit              | [3Z9cCZHui2NTXmSo66CugG4fX764B3mtd1yBSZCS9oyBxyXkM6bqbWesunndpYK397XC7VFWjYMy2WTum2xDMNRk](https://explorer.solana.com/tx/3Z9cCZHui2NTXmSo66CugG4fX764B3mtd1yBSZCS9oyBxyXkM6bqbWesunndpYK397XC7VFWjYMy2WTum2xDMNRk?cluster=devnet) |
| 15  | SPL   | multi-input | deposit              | [2f9nEJmHKDC8GdKyrT7uj3YsgzXptMFRGxdNgopQEoKYdc7gEh5TrN9VkA2hLFmr4unv78MTpAbdeNToF4JdQp6E](https://explorer.solana.com/tx/2f9nEJmHKDC8GdKyrT7uj3YsgzXptMFRGxdNgopQEoKYdc7gEh5TrN9VkA2hLFmr4unv78MTpAbdeNToF4JdQp6E?cluster=devnet) |
| 16  | SPL   | multi-input | transfer             | [3xqFL2hajkMwQsiAivJWjvz4RYAiLeVKiqFaqTVC9GaDDyDDgUkusn5Us7VTaswMsgQ38RmN3gD72fi1jVh7wEZi](https://explorer.solana.com/tx/3xqFL2hajkMwQsiAivJWjvz4RYAiLeVKiqFaqTVC9GaDDyDDgUkusn5Us7VTaswMsgQ38RmN3gD72fi1jVh7wEZi?cluster=devnet) |
| 17  | SPL   | multi-input | withdraw (sender)    | [2ud1a1mXDCJk77R49LWHojwGixMduhDZwu3AV5YXbkpB4dET5LPi64CFUQD3auA8xEEdngdAzgW1J5DqBWVGWEge](https://explorer.solana.com/tx/2ud1a1mXDCJk77R49LWHojwGixMduhDZwu3AV5YXbkpB4dET5LPi64CFUQD3auA8xEEdngdAzgW1J5DqBWVGWEge?cluster=devnet) |
| 18  | SPL   | multi-input | withdraw (recipient) | [5sZww3mbSg1kgDiHvYuE5rBbbgj9QBj2ABP1s8ugScvDgr9DwwRFYHKHtY3MbBFoPYg41Q58urCbwDQspLTkSnxk](https://explorer.solana.com/tx/5sZww3mbSg1kgDiHvYuE5rBbbgj9QBj2ABP1s8ugScvDgr9DwwRFYHKHtY3MbBFoPYg41Q58urCbwDQspLTkSnxk?cluster=devnet) |

**Run Conditions**

- Date: 2026-11-03.
- Runtime: Node.js 24.21.0.
- Privy Node SDK: 0.1.0-alpha.2.
- Solana Kit: 8.6.0.
- Privacy SDK: 0.8.0-alpha, installed from Git commit `c4062d3860965855e47268f61b01a27e774096eb`, package `sdk-libs/ts`.
- TEE: AWS Nitro, verified with the SDK's signed Devnet policy.
- Two Privy Solana wallets controlled by the same owner authorization key.
- Existing shielded public-key registrations were verified and reused. A fresh nine-decimal classic SPL mint was created for this run.
- Setup transactions, previous attempts, and cleanup transactions are excluded from these measurements.
- Missing and incorrect owner authorization were rejected. An unregistered recipient was rejected without spending the private balance.
- Exact public SPL balance changes, SOL interface balance changes, indexer-synchronized private balances, and final zero private balances were asserted.
