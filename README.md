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
| [`deposit_transfer_withdraw`](rust-client/examples/deposit_transfer_withdraw.rs) | Deposit, private transfer, and withdraw. |

### [TypeScript client](typescript-client/README.md)

|  |  |
|---------|-------------|
| [`deposit_transfer_withdraw`](typescript-client/examples/deposit_transfer_withdraw.ts) | Deposit, private transfer, and withdraw. |

### Program examples

|  |  |
|---------|-------------|
| [`swap-program/`](swap-program/) | A confidential swap between a maker and a taker. |
| [`escrow-program/`](escrow-program/) | A timelock escrow on SPP: lock a private balance until a deadline, then release or reclaim. |

## Documentation
- [Demo](https://helius-privacy-demo.fly.dev/)
- [Documentation](https://helius.dev/docs/privacy)
- [Source Code](https://github.com/helius-labs/zolana)
