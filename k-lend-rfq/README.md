# Private kVault Deposits

Deposit into and withdraw from a Kamino kVault with a private balance, without
revealing amounts. The user swaps private USDC for private kVault shares with a
market maker through a private RFQ. The market maker deposits into the vault in
aggregate on its own schedule. Withdrawals work the same way in reverse. No
custom program is involved: the vault is the unmodified kVault program.

See [`spec.md`](spec.md) for the full design.

## Layout

- [`sdk/`](sdk/): client library. Pairs, quotes, the two transfers of
  a swap, and the user's checks before signing.
- [`market-maker/`](market-maker/): the market maker. Quotes, fills,
  inventory, and rebalancing against kVault. Localnet tests in `tests/`.
- [`test-utils/`](test-utils/): localnet setup, test wallets and users.
- [`examples/`](examples/): [`deposit_and_withdraw`](examples/examples/deposit_and_withdraw.rs),
  one private deposit and withdrawal through the market maker.

## Run

The Zolana crates come from the `v0.4.0-alpha` tag of
[helius-labs/zolana](https://github.com/helius-labs/zolana). The example and the
tests boot their own localnet: surfpool with the shielded pool and both Kamino
programs, Photon, and a prover.

1. Download the `zolana`, `prover`, `photon` and `shielded_pool_program` assets
   for your platform from the
   [`v0.4.0-alpha` release](https://github.com/helius-labs/zolana/releases/tag/v0.4.0-alpha),
   and `surfpool` from the
   [`v1.6.0-light` release](https://github.com/Lightprotocol/surfpool/releases/tag/v1.6.0-light).
2. Dump the Kamino kVault and lending programs from mainnet into
   `target/deploy` (needs the Solana CLI):

   ```bash
   scripts/dump-kamino.sh
   ```

   The script skips a program whose `.so` is already in `target/deploy`, so
   delete the file to dump a newer mainnet deployment locally. It dumps
   through the public mainnet RPC unless `KAMINO_DUMP_RPC_URL` names another.
   CI caches the dumped programs under `KAMINO_PROGRAMS_CACHE_VERSION` in
   `.github/workflows/examples.yml`; bump it to pick up a new mainnet
   deployment.

3. Point the harness at the binaries and run, from `k-lend-rfq/`:

   ```bash
   export ZOLANA_CLI_BIN=/path/to/zolana
   export PROVER_BIN=/path/to/prover
   export ZOLANA_PHOTON_BIN=/path/to/photon
   export SURFPOOL_BIN=/path/to/surfpool
   export SHIELDED_POOL_PROGRAM_PATH=/path/to/shielded_pool_program.so

   cargo run -p k-lend-rfq-example --example deposit_and_withdraw
   cargo test -p k-lend-market-maker --tests
   ```

The `mainnet_vault` test snapshots a mainnet Kamino USDC vault into the
localnet and needs mainnet access: set `KAMINO_MAINNET_RPC_URL` to a mainnet
RPC URL, otherwise it skips.

Each test boots its localnet on ports derived from its test number; set
`ZOLANA_PORT_OFFSET` to shift all of them, for example when another checkout
runs the tests at the same time or a port is taken.

On first start the prover downloads the proving keys it is missing into
`~/.config/zolana/proving-keys`, the directory `zolana test-env` uses. Set
`ZOLANA_PROVER_KEYS_DIR` to keep them elsewhere.
