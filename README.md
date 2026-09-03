# Examples for private Solana rings

### [Rust client](rust-client/README.md)

|  |  |
|---------|-------------|
| [`create_private_wallet`](rust-client/examples/create_private_wallet.rs) | Create a private wallet and register its Solana address. |
| [`deposit_transfer_withdraw`](rust-client/examples/deposit_transfer_withdraw.rs) | Deposit, private transfer, and withdraw. |
| [`sync_balance`](rust-client/examples/sync_balance.rs) | Read the private SOL and SPL balances. |
| [`read_history`](rust-client/examples/read_history.rs) | Read the private transaction history. |

### [TypeScript client](typescript-client/README.md)

|  |  |
|---------|-------------|
| [`create_private_wallet`](typescript-client/examples/create_private_wallet.ts) | Create a private wallet and register its Solana address. |
| [`deposit_transfer_withdraw`](typescript-client/examples/deposit_transfer_withdraw.ts) | Deposit, private transfer, and withdraw. |
| [`sync_balance`](typescript-client/examples/sync_balance.ts) | Read the private SOL and SPL balances. |
| [`read_history`](typescript-client/examples/read_history.ts) | Read the private transaction history. |

### Program examples

|  |  |
|---------|-------------|
| [`swap-program/`](swap-program/) | A confidential swap between a maker and a taker. |
| [`escrow-program/`](escrow-program/) | A timelock escrow on SPP: lock a private balance until a deadline, then release or reclaim. |

## Documentation
- [Demo](https://helius-privacy-demo.fly.dev/)
- [Documentation](https://helius.dev/docs/privacy)
- [Source Code](https://github.com/helius-labs/zolana)
