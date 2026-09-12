# Embedded private wallet

A minimal React + Vite wallet using Turnkey for login and Solana transaction signing, and Turnkey Verifiable Compute (TVC) for the Zolana SDK’s `WalletKeys`. The layout and transaction receipts are preserved from the Privy example.

## Run

Use Node 24+ and pnpm 11.18.0.

From the repository root:

```bash
cd embedded-private-wallet
pnpm install
cp .env.example .env
# Set VITE_API_KEY to a Helius project with embedded wallets enabled.
pnpm dev
```

Open `http://127.0.0.1:5173/`. The example’s `.env` is ignored by Git. The Helius browser key is public configuration; never add Turnkey operator credentials or app secrets to `VITE_` variables. A previous Privy app ID in `.env` is unused.

The local Vite proxy (also enabled for `pnpm preview`) connects only to the selected devnet backend, `https://i6npwfd4mh.eu-west-1.awsapprunner.com`. TVC and private indexer/prover requests use this proxy. Normal Solana RPC uses the configured devnet connection. This is a local example: deploying the static output requires an equivalent server proxy; `dist` alone does not supply one.

## Wallet flow

1. **Sign in with Turnkey.** Login alone does not enroll a TVC wallet, bootstrap keys, or submit registration.
2. **Fund the displayed devnet address.** Fund the amount you want to deposit, plus registration rent and transaction fees. This is a separate wallet from the previous Privy account; its funds and registration are not migrated.
3. **Activate private wallet.** The app verifies the pinned release policy, PCRs and Boot Proof, enrolls the browser authorizer, reconciles Turnkey grants, restores or bootstraps the private identity, checks the onchain registry, registers when needed, and syncs balances.
4. **Test the actions.** Enter an amount in SOL for deposit, transfer to a registered recipient, or withdrawal to the connected wallet. Each action remembers its amount while switching tabs; defaults are 0.01 / 0.003 / 0.003 SOL. Amounts support up to 9 decimal places and are converted exactly to lamports. Turnkey signs the SDK-built Solana transaction. The returned message and signature are verified before submission.
5. **Refresh balances.** Refresh reuses the active TVC context; it does not repeat enrollment or bootstrap. A confirmed transaction retains its explorer receipt if the following sync fails. Refresh before another action.
6. **Reload and activate again.** The same Turnkey account restores its saved identity and sealed seed without another bootstrap. Registration is checked onchain rather than trusted from the local flag.

Account changes and logout cancel the active session and clear balances, receipts and errors. Already broadcast transactions cannot be cancelled.

## Progress and motion

Transfer follows the demo’s **Preparing → Proving → Sending → Confirmed** milestones. Preparing includes recipient lookup and transaction preparation. Proving begins at `WalletKeys.prove`; Sending includes wallet approval and broadcast. Confirmation is reported only after submission confirms. Balance sync keeps controls disabled afterward, and a sync failure retains the confirmed explorer receipt. Timings measure actual stage boundaries, not simulated progress.

The centered **View transaction** link is the final receipt. There is no additional “Transaction confirmed” or “Transfer complete” line. The canonical Launch Demo link is https://helius.dev/privacy/demo.

| Transition | Behavior |
|---|---|
| Sign in / sign out, activation / action form | Panel height eases over 340ms; new content fades in. |
| Deposit / Transfer / Withdraw | Selection slides over 480ms; recipient and destination content resize smoothly. |
| Progress, errors, retry, receipt appearance / removal | Measured height expands and collapses; content fades in. |
| Balance refresh, address expansion, copy feedback | Values fade; height changes flow into the surrounding layout. |
| Hover, focus, disabled controls | Short color, border, and opacity transitions. |
| Reduced motion | Height changes apply immediately; decorative transitions and animations are disabled. |

Height changes use the demo’s `cubic-bezier(0.32, 0.72, 0, 1)` easing and retarget from the visible height if interrupted. Motion never delays signing guards, validation, or session cleanup. Turnkey’s login modal keeps its existing SDK behavior.

The isolated motion preview was checked at 390px and 1440px, including action tabs, activation, progress and the centered receipt. Recorded frames showed intermediate panel/button positions without horizontal overflow. These previews use simulated operations and do not establish transaction success.

## Storage and security boundary

TVC keeps the long-lived viewing and nullifier keys out of the app’s normal wallet context. The browser’s IndexedDB stores the public identity, signed wallet descriptor, enclave-sealed seed, and nonexportable P-256/AES CryptoKeys for the browser authorizer. Records are separated by app, Turnkey organization, wallet and account. The known public identity is saved separately so recovery cannot silently adopt another identity. No derivation signature is persisted or logged.

This integration uses TVC as currently implemented. Its external prover receives plaintext proof witness material, including the nullifier secret. Turnkey’s bootstrap approval API can return signature material; the copied approval helper discards that response. This example does **not** claim that all secrets remain exclusively inside the enclave.

Corrupted state, changed client bindings, and conflicting identities fail explicitly. The app does not silently erase state or overwrite an existing registry entry. Preserve the known public identity when diagnosing recovery errors; deleting browser data is not an identity migration.

## Dependencies and references

Zolana is pinned to `0.1.6-alpha`. TVC is unpublished, and the published wallet-kit 1.1.0 lacks the TVC session APIs used here. The local `vendor/` archives contain builds from these inspected revisions:

- [zolana-tvc](https://github.com/helius-labs/zolana-tvc/tree/35dafa8ad8afcbdb6099517f8b4f9db899a112a2)
- [wallet-kit browser integration](https://github.com/helius-labs/wallet-kit/tree/c00f7a5e5ee7a2e2013c8301c762f870a99f8ee7)

`vendor/README.md`, `vendor/SHA256SUMS`, and `scripts/rebuild-vendor.sh` record provenance and rebuild commands. Only the React provider and signing/enrollment helpers are reused; the app does not adopt Next.js or the full demo UI.

The interface retains the existing system font, balance hierarchy, 44px controls, keyboard focus styles, and status feedback, informed by Apple’s [Typography](https://developer.apple.com/design/human-interface-guidelines/typography), [Layout](https://developer.apple.com/design/human-interface-guidelines/layout), and [Buttons](https://developer.apple.com/design/human-interface-guidelines/buttons) guidance.

## Tests and validation

```bash
pnpm check
pnpm test
pnpm build
```

Tests cover explicit activation and duplicate guards, canceled sessions, registry conflicts, Turnkey transaction integrity, bootstrap activity selection, enrollment errors, IndexedDB persistence, recipient validation, balances and retained receipts. The derivation compatibility test remains isolated from the browser wallet flow. The optional `pnpm test:integration` uses a local CLI keypair and is **not** evidence of Turnkey/TVC browser success.

Validation recorded during migration:

- The Privy checkpoint is commit `0207237`. Its message signing and onchain registration worked, but private sync failed with `WALLET_SYNC: CLIENT_REQUEST: API_REQUEST`; private actions were not verified.
- The migrated app type-checks, all 105 tests across 16 files pass, and its production build passes. Upstream WASM URL warnings and large bundle warnings remain.
- Live TVC attestation passed through the local proxy against the independently pinned policy, PCRs and Boot Proof.
- The real Turnkey email login dialog opens successfully with the configured Helius project. Wallet and login layouts were inspected at 390px and 1440px without horizontal overflow or page errors. Connected and activated layouts were inspected at both widths using isolated visual fixtures, not real transaction results.
- The production preview proxy returns the pinned release and rejects an untrusted Origin with HTTP 403.
- **Live transaction acceptance is partial.** The signed-in Turnkey wallet restored after reload, activation completed, and a user-submitted 0.001 SOL deposit appeared with a confirmed receipt and refreshed public/private balances. The live UI also showed a user-submitted self-transfer completing with stage timings, an explorer receipt, and refreshed balances. Withdrawal and transfer to a different registered recipient remain unverified. Automatic approval review requires the user to perform wallet activation and transaction clicks; the agent can inspect the resulting receipts and balances.
