# Validation — 2026-11-02

**Privy end-to-end validation is pending an authenticated test app.**

Found the maintained [Privy Solana starter](https://github.com/privy-io/examples/tree/main/examples/privy-next-solana)
and two local Privy examples under `Workspace/examples-light-token/toolkits/sign-with-privy`
and `Workspace/zk-compression/examples-zk-compression/privy`.
Neither local example has a configured `.env` file.
The Privy dashboard requires sign-in.

| Check                                                       | Result                                   |
| ----------------------------------------------------------- | ---------------------------------------- |
| TypeScript check                                            | Passed                                   |
| Production browser build                                    | Passed                                   |
| Local signing tests                                         | 4 passed                                 |
| Canonical-message derivation and recovery on Devnet         | Passed with local test signers           |
| Register both private wallets                               | Passed on Devnet with local test signers |
| Deposit 0.01 SOL                                            | Passed on Devnet with local test signers |
| Confidential transfer of 0.003 SOL and recipient decryption | Passed on Devnet with local test signers |
| Recipient withdrawal of 0.003 SOL                           | Passed on Devnet with local test signers |
| Sender withdrawal of remaining 0.007 SOL                    | Passed on Devnet with local test signers |
| Final private balances                                      | Both zero                                |
| Privy server-wallet signing                                 | Not run; app credentials unavailable     |
| Privy browser login and embedded-wallet signing             | Not run; app authentication unavailable  |

The live run used `@heliuslabs/zolana@0.7.0-alpha`, the repository's Devnet E indexer and prover,
and its AWS Nitro attestation policy. The RPC returned Devnet genesis hash
`EtWTRABZaYq6iMfeYKouRu166VU2xqa1wcaWoxPkrZBG`.
The older TypeScript example RPC key returned HTTP 401; the demo's local configuration worked.

| Step                  | Confirmed slot | Transaction                                                                                                                                         |
| --------------------- | -------------- | --------------------------------------------------------------------------------------------------------------------------------------------------- |
| Deposit               | 517890136      | [ysZLLK6c…](https://explorer.solana.com/tx/ysZLLK6cCmhxt93tchWQkTna5F6Y6vFb89jyByijZLn7sbC2ahzchsAfoL1NDbTf3pE6hBiF4ZyqCN4w1sHFNad?cluster=devnet)  |
| Confidential transfer | 517890146      | [3rkRgZEf…](https://explorer.solana.com/tx/3rkRgZEfhxfVTukGF4QoLJ5ZRUQgmHiKexKCsXcjLFzq59pm24xfioEFmLzMdZ56TMYqM8p3NYz9DV9tfpcu9vid?cluster=devnet) |
| Recipient withdrawal  | 517890156      | [649uEFBb…](https://explorer.solana.com/tx/649uEFBbge8KxeoJBpS3ZNn2cEvhmL1Ji7gb3bZPKBocPMcnMUoCHSY2wWMZhtySwdGhWA659LctFUtrqeR4taqP?cluster=devnet) |
| Sender withdrawal     | 517890165      | [cH4qJ5FY…](https://explorer.solana.com/tx/cH4qJ5FYzLm3hvxn3NFEQeQ4BthK6d1GsU27Nsjd9vDehyfuN5tFqK1c2ettnAJDd2Qoi6v7hzevgaJMc31xgkq?cluster=devnet)  |

The local signing tests check wallet recovery, rejection of a signature from another wallet,
transaction signing, and rejection of an unsigned response.
They use local keys and do not establish Privy compatibility.
