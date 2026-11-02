# Privy confidential transfers on Devnet

React starter with Privy Solana embedded wallets and `@heliuslabs/zolana`.
The same SDK flow also runs with Privy server wallets for automated validation.

The example registers two wallets, deposits 0.01 SOL, transfers 0.003 SOL confidentially,
decrypts the recipient's balance, and withdraws both private balances to their public wallets.
A confidential transfer reveals sender and recipient; it encrypts the asset and amount.
Deposits and withdrawals reveal the asset and amount.

## Setup

Use Node.js 24 or later.

```bash
npm install
npm install --prefix validation
cp .env.example .env
```

Set `API_KEY` to a Helius Devnet RPC key.
Set `VITE_PRIVY_APP_ID` to your Privy App ID.
In the Privy dashboard, enable Solana embedded wallets and allow `http://localhost:5173`.

```bash
npm run dev
```

Open the local URL, log in, and create a second Solana embedded wallet.
Fund both displayed addresses with at least 0.05 Devnet SOL each, then select **Validate on Devnet**.
The app asks Privy to sign the SDK's canonical derivation message and each Solana transaction.
It never exports the Privy signing key.
Privacy keys derived from the message signature stay in the browser; the SDK uses the attested prover policy from `../devnet-e-tee-policy.json`.

The local Vite proxy reads `API_KEY` on the server. It is not a `VITE_` variable.
The browser starter uses this proxy; configure an RPC proxy for another hosting environment.

## Automated Privy validation

Set `PRIVY_APP_ID` and `PRIVY_APP_SECRET` in `.env`.
Use a dedicated test app whose wallet API can create and sign with app-owned Solana wallets.
The secret belongs only in the Node process.

```bash
npm run validate:devnet
```

The script creates two Privy server wallets, saves their IDs and public addresses in `wallets.local.json`,
and funds each with 0.05 Devnet SOL from `ZOLANA_PAYER_KEYPAIR` (default `~/.config/solana/id.json`).
The payer needs at least 0.101 Devnet SOL.
Successful validation saves confirmed transaction signatures and slots to `validation.local.json`.
The funded public balances remain in the two test wallets.

Server-wallet validation proves the Privy wallet API signing path.
The browser login and embedded-wallet path needs its own live run.

## Checks

```bash
npm run check
npm test
npm run build
```

The signer tests use local Ed25519 keys. They do not call Privy.
`npm run validate:protocol` runs the same live Devnet flow with local test signers.
It saves recovery seeds to the ignored, owner-readable `protocol-seeds.local.json` before funding the test wallets.
It refuses to overwrite that file. Its results do not validate Privy signing.

See [VALIDATION.md](VALIDATION.md) for the latest recorded result.

## References

- [Privy Solana starter](https://github.com/privy-io/examples/tree/main/examples/privy-next-solana): provider configuration, message signing, and transaction signing.
- [Privy Node starter](https://github.com/privy-io/examples/tree/main/privy-node-starter): server client configuration.
- [`typescript-client/examples/deposit_transfer_withdraw.ts`](../typescript-client/examples/deposit_transfer_withdraw.ts): deposit, proof, transfer, indexer freshness, and withdrawal sequence.
- [Zolana wallet adapter example](https://github.com/helius-labs/zolana-examples/tree/feat/sign-with-wallet-adapter/embedded-private-wallet/src/lib): Kit wire-transaction adapter and version 0 transaction composition.
- [Zolana SDK `LocalKeys`](https://github.com/helius-labs/zolana/blob/v0.7.0-alpha/sdk-libs/ts/src/client/keys.ts): signature-based privacy-key derivation.

Privy's Node SDK currently declares a Solana Kit 5 peer dependency.
Its validation dependencies are installed separately from the browser and Zolana SDK, which use Kit 8.
The integration exchanges transaction bytes between them.
