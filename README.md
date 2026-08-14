# Examples for private Solana rings

Rings enable confidential transfers for SOL and any SPL asset, while keeping execution on Solana and custody with the user:
* Users hold private balances of SOL or any SPL asset in a Ring. This balance is encrypted onchain.
* Funds move in a single Solana transaction between public and private balances via deposit, withdrawal, or private transfer.
* A zero-knowledge proof attests that the sender owns and can spend the private balance used by a transfer.
* The Solana Privacy Program verifies the ZK proof without revealing asset and amount.


### Privacy guarantees

| | Default Ring | Custom Ring |
| --- | --- | --- |
| Amount | Private | Private |
| Asset | Private | Private |
| Sender | Public | Public | Private |
| Recipient | Public | Public | Private |
| Access | Permissionless | Custom policy and compliance controls |

### [Rust client](rust-client/README.md)

|  |  |
|---------|-------------|
| [`deposit`](rust-client/examples/deposit_instruction.rs) | Move tokens from a public to a private balance. |
| [`transfer`](rust-client/examples/transfer_instruction.rs) | Transfer privately between private balances. |
| [`withdraw`](rust-client/examples/withdraw_instruction.rs) | Move tokens from a private to a public balance. |

### [TypeScript client](typescript-client/README.md)

|  |  |
|---------|-------------|
| [`deposit`](typescript-client/examples/deposit_instruction.ts) | Move SOL from a public to a private balance. |
| [`transfer`](typescript-client/examples/transfer_instruction.ts) | Transfer SOL privately between private balances. |
| [`withdraw`](typescript-client/examples/withdraw_instruction.ts) | Move SOL from a private to a public balance. |

### Program examples

|  |  |
|---------|-------------|
| [`swap-program/`](swap-program/) | A confidential swap between a maker and a taker. |
| [`escrow-program/`](escrow-program/) | A timelock escrow on SPP: lock a private balance until a deadline, then release or reclaim. |

## Documentation
- [Demo](https://helius-privacy-demo.fly.dev/)
- [Documentation](https://helius.dev/docs/privacy)
- [Source Code](https://github.com/helius-labs/zolana)
