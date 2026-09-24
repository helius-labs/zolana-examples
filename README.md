# Examples for private Solana rings

### [Rust client](rust-client/README.md)

|  |  |
|---------|-------------|
| [`register_private_wallet`](rust-client/examples/register_private_wallet.rs) | Register a private wallet. |
| [`deposit_transfer_withdraw`](rust-client/examples/deposit_transfer_withdraw.rs) | Deposit, private transfer, and withdraw. |
| [`deposit_with_interface_setup`](rust-client/examples/deposit_with_interface_setup.rs) | Create a token interface and deposit in one transaction. |
| [`sync_balance`](rust-client/examples/sync_balance.rs) | Read the private SOL and SPL balances. |
| [`read_history`](rust-client/examples/read_history.rs) | Read the private transaction history. |

### [TypeScript client](typescript-client/README.md)

|  |  |
|---------|-------------|
| [`register_private_wallet`](typescript-client/examples/register_private_wallet.ts) | Register a private wallet. |
| [`deposit_transfer_withdraw`](typescript-client/examples/deposit_transfer_withdraw.ts) | Deposit, private transfer, and withdraw. |
| [`deposit_with_interface_setup`](typescript-client/examples/deposit_with_interface_setup.ts) | Create a token interface and deposit in one transaction. |
| [`sync_balance`](typescript-client/examples/sync_balance.ts) | Read the private SOL and SPL balances. |
| [`read_history`](typescript-client/examples/read_history.ts) | Read the private transaction history. |

### Program examples

|  |  |
|---------|-------------|
| [`swap-program/`](swap-program/) | A confidential swap between a maker and a taker. |
| [`escrow-program/`](escrow-program/) | A timelock escrow on SPP: lock a private balance until a deadline, then release or reclaim. |

#### Deploy a program example to devnet

The committed verifying keys come from an insecure test setup: the recommended
setup for integration tests, but anyone can forge proofs against them. A devnet
deployment needs its own keys. Run these from the example's directory
(`swap-program/` or `escrow-program/`):

| Example | Setup binary | Circuits | Program binary |
|---|---|---|---|
| swap | `cargo run --manifest-path prover/Cargo.toml --bin swap-prover-setup --` | `make take cancel take_verifiable_encryption` | `program/target/deploy/swap_program.so` |
| escrow | `cargo run -p timelock-escrow-prover --bin timelock-escrow-prover-setup --` | `escrow withdraw` | `target/deploy/timelock_escrow_program.so` |

1. Give the program its own id: create a keypair with
   `solana-keygen new -o program-keypair.json` and put its address in the
   `declare_id!` of `program/src/lib.rs`.
2. Set up keys from system randomness, without `--insecure-test-keys`. Each run
   writes `build/gnark/<circuit>/{pk,vk}.bin` and the circuit's production
   verifying key into the program:

   ```bash
   for c in <circuits>; do
     <setup binary> "$c" "build/gnark/$c" --rust-vk "program/src/verifying_keys/$c.rs"
   done
   shasum -a 256 build/gnark/*/pk.bin > devnet-keys.CHECKSUM
   ```

   Keep the `pk.bin` files: the prover loads them from `build/gnark/`, and
   clients that prove for the deployment need the same files. The setup keeps
   its randomness in memory, so a second run produces different keys, which the
   deployed program rejects. A single-party setup is only as trustworthy as the
   machine that ran it, so do not use one for a deployment that holds real
   value.
3. Build without the `insecure-test-setup` feature, so a test key left in the
   program fails to compile:

   ```bash
   cargo build-sbf --manifest-path program/Cargo.toml --tools-version v1.54 -- --no-default-features --features bpf-entrypoint
   ```

4. Deploy, and check the verifying keys before and after with the
   [`zolana` CLI](https://github.com/helius-labs/zolana/blob/main/cli/README.md):

   ```bash
   zolana vks check --so <program binary> --expect devnet-keys.CHECKSUM
   solana program deploy <program binary> --program-id program-keypair.json --url devnet
   zolana vks check --program-id <program id> --rpc-url https://api.devnet.solana.com --expect devnet-keys.CHECKSUM
   ```

   `zolana vks check` fails if the program embeds any insecure test setup, and
   `--expect` fails if one of your proving keys is missing from it.

## Documentation
- [Demo](https://helius-privacy-demo.fly.dev/)
- [Documentation](https://helius.dev/docs/privacy)
- [Source Code](https://github.com/helius-labs/zolana)
