# Private Solana Ring Examples

Rings enable confidential transfers for SOL and any SPL asset, while keeping execution on Solana and custody with the user:
* Users hold private balances of SOL or any SPL asset in a Ring. This balance is encrypted onchain.
* Funds move in a single Solana transaction between public and private balances via deposit, withdrawal, or private transfer.
* For every private transfer, a zero-knowledge proof attests a user owns and can transfer tokens from their private balance.
* The Solana Privacy Program verifies the ZK proof without revealing asset and amount.


### Privacy Guarantees

| | Default Ring | Custom Ring |
| --- | --- | --- |
| Amount | Private | Private |
| Asset | Private | Private |
| Sender | Public | Public | Private |
| Recipient | Public | Public | Private |
| Access | Permissionless | Custom policy and compliance controls |

### [Rust Client](rust-client/README.md)

|  |  |  |  |
|---------|-------------|---------|---------|
| [`transfer`](rust-client/examples/transfer.rs) | Transfer privately between private balances. | [Action](rust-client/examples/transfer.rs) | [Instruction](rust-client/examples/transfer_instruction.rs) |
| [`deposit`](rust-client/examples/deposit.rs) | Move tokens from a public to a private balance. | [Action](rust-client/examples/deposit.rs) | [Instruction](rust-client/examples/deposit_instruction.rs) |
| [`withdraw`](rust-client/examples/withdraw.rs) | Move tokens from a private to a public balance. | [Action](rust-client/examples/withdraw.rs) | [Instruction](rust-client/examples/withdraw_instruction.rs) |
| [`create_private_wallet`](rust-client/examples/create_private_wallet.rs) | Create a private wallet. | [Action](rust-client/examples/create_private_wallet.rs) |  |
| [`sync_balance`](rust-client/examples/sync_balance.rs) | Read a wallet's private balance. | [Action](rust-client/examples/sync_balance.rs) |  |

### Program examples

|  |  |
|---------|-------------|
| [`swap-program/`](swap-program/) | A confidential swap between a maker and a taker. |
| [`escrow-program/`](escrow-program/) | A timelock escrow on SPP: lock a private balance until a deadline, then release or reclaim. |

## Documentation

- [Documentation](https://helius.dev/docs/privacy)
- [Source Code](https://github.com/helius-labs/zolana)
