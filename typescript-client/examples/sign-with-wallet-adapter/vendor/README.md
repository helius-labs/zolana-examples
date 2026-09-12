# Pinned browser packages

These packages are built from inspected source because TVC is unpublished and the public wallet-kit 1.1.0 does not expose the TVC session APIs.

| Package | Repository | Revision |
| --- | --- | --- |
| `@zolana/tvc-wallet` | https://github.com/helius-labs/zolana-tvc | `35dafa8ad8afcbdb6099517f8b4f9db899a112a2` |
| `helius-wallet-kit` | https://github.com/helius-labs/wallet-kit | `c00f7a5e5ee7a2e2013c8301c762f870a99f8ee7` |

Build with Node 24 and the upstream pinned pnpm versions. Run `bash scripts/rebuild-vendor.sh`; review checksum changes before updating the lockfile. `SHA256SUMS` records the shipped archives. Package timestamps may differ on a rebuild.

The TVC build runs its production-boundary check. This app never imports the `/testing` entry point. Licenses are included alongside the archives. The TVC policy, bootstrap approval helpers and transaction signer (and corresponding tests) come from the wallet-kit revision above. Browser storage uses the TVC package's strict parsers and nonexportable CryptoKeys.
