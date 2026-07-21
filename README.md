# Private Solana Ring Examples

Helius makes privacy on Solana accessible for SOL and any SPL asset via simple APIs.
Privacy Rings allow for confidential and anonymous transfers, while keeping execution on Solana and custody with the user:
* Users hold private balances of SOL or any SPL asset in a Ring. This balance is encrypted onchain.
* For every private transfer, a zero-knowledge proof (ZKP) attests a user owns and can transfer tokens from their private balance. Funds move in a single Solana transaction between public and private balances in Helius Privacy Rings via deposit, withdrawal, or private transfer.
* The Solana Privacy Program verifies the ZK proof without revealing asset and amount in confidential rings, or anything in anonymous rings.

The level of privacy depends on the Ring a user holds her private balance:

| | Default Ring (confidential) | Custom Ring (confidential) | Custom Ring (anonymous) |
| --- | --- | --- | --- |
| Amount | Private | Private | Private |
| Asset | Private | Private | Private |
| Sender | Public | Public | Private |
| Recipient | Public | Public | Private |
| Access | Permissionless | Custom policy and compliance controls | Custom policy and compliance controls |

### Rust Client

Self-contained Rust binaries that drive the `zolana-client` SDK against a local
validator, Photon indexer, and prover. They live in
[`rust-client/`](rust-client/) — see its [README](rust-client/README.md) to
build the prerequisites and run them.

|  |  |
|---------|-------------|
| [`create_private_wallet`](rust-client/examples/create_private_wallet.rs) | Create and register a wallet for a private balance. |
| [`deposit`](rust-client/examples/deposit.rs) | Move public tokens into a private balance. |
| [`transfer`](rust-client/examples/transfer.rs) | Send a value privately between two private balances. |
| [`withdraw`](rust-client/examples/withdraw.rs) | Withdraw a private balance back to a public account. |
| [`sync_balance`](rust-client/examples/sync_balance.rs) | Read a wallet's private balance from the indexer. |

### Program examples

ZK programs built on the Solana Privacy Program (SPP), each verifying a proof of
its own rules and delegating the confidential transfer to SPP.

|  |  |
|---------|-------------|
| [`swap-program/`](swap-program/) | A confidential swap between a maker and a taker on SPP. |
| [`escrow-program/`](escrow-program/) | A timelock escrow on SPP: lock a private balance until a deadline, then release or reclaim. |

## Documentation

- [Documentation](https://helius.dev/docs/privacy)
- [Source Code](https://github.com/helius-labs/zolana)
- [AI Skill](https://example.com/zolana-ai-skill)
