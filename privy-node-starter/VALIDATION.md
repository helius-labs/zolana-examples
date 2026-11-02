# Validation — 2026-11-02

**Privy server-wallet signing and confidential transfers passed end to end on Devnet.**

Used the maintained [Privy Solana starter](https://github.com/privy-io/examples/tree/main/examples/privy-next-solana)
and [Privy Node starter](https://github.com/privy-io/examples/tree/main/privy-node-starter) as references.
The live run used the configured test app and real Privy wallet API calls.
Privy signed the canonical derivation messages and every registration, deposit, transfer, and withdrawal transaction.
The second derivation signature recovered the same sender privacy keys.

| Check                                                       | Result                            |
| ----------------------------------------------------------- | --------------------------------- |
| TypeScript check                                            | Passed                            |
| Production browser build                                    | Passed                            |
| Local signing tests                                         | 4 passed                          |
| Canonical-message derivation and recovery on Devnet         | Passed with real Privy signatures |
| Register both private wallets                               | Passed with real Privy signatures |
| Deposit 0.01 SOL                                            | Passed with real Privy signatures |
| Confidential transfer of 0.003 SOL and recipient decryption | Passed with real Privy signatures |
| Recipient withdrawal of 0.003 SOL                           | Passed with real Privy signatures |
| Sender withdrawal of remaining 0.007 SOL                    | Passed with real Privy signatures |
| Final private balances                                      | Both zero                         |
| Privy server-wallet signing                                 | Passed                            |
| Privy browser login and embedded-wallet signing             | Not run                           |

The live run at 18:09 UTC used `@privy-io/node@0.37.0`, `@heliuslabs/zolana@0.7.0-alpha`, the repository's Devnet E indexer and prover,
and its AWS Nitro attestation policy. The RPC returned Devnet genesis hash
`EtWTRABZaYq6iMfeYKouRu166VU2xqa1wcaWoxPkrZBG`.
The older TypeScript example RPC key returned HTTP 401; the demo's local configuration worked.

| Step                  | Confirmed slot | Transaction                                                                                                                                         |
| --------------------- | -------------- | --------------------------------------------------------------------------------------------------------------------------------------------------- |
| Deposit               | 517940423      | [52GwJ9fw…](https://explorer.solana.com/tx/52GwJ9fwBgdB875g6iHpmiKk5LwqBC6r9i1GyGLZqf6eP8tj9jMHfDULfnWCNTNWzRBNkv2NqzZ5WEPEMKnPsKMi?cluster=devnet) |
| Confidential transfer | 517940432      | [5KGFPrm3…](https://explorer.solana.com/tx/5KGFPrm3BzunAwUgcXUhKMg9NxFmsWFpEFavVFjoiea9zKK8iXpNzKcDxFX2BRLfkRm4qnaEEqGAXTzDo5meuCoe?cluster=devnet) |
| Recipient withdrawal  | 517940443      | [2QUj2XLP…](https://explorer.solana.com/tx/2QUj2XLPT6RZq9xpHSkz9ccAcCZg58hDM3BTYsyJrBDrHQZcd5f2bYkV8fJFQsyiTwtFvC5tNq9FYWhTqNStbNju?cluster=devnet) |
| Sender withdrawal     | 517940452      | [5yDu6TBE…](https://explorer.solana.com/tx/5yDu6TBEEEjkpDWjBaRmQiFgwTTikKUckMEYxog95rLJW641ucw6bRt5w7UrEdPaSBjXdAjZheXBNHkCuDuHEW2z?cluster=devnet) |

The local signing tests check wallet recovery, rejection of a signature from another wallet,
transaction signing, and rejection of an unsigned response.
They use local keys and do not establish Privy compatibility.

The real Privy run is recorded separately in [devnet-privy.json](validation/devnet-privy.json).
The earlier local-signer run remains in [devnet-protocol.json](validation/devnet-protocol.json).
The browser embedded-wallet path still needs its own live run.
