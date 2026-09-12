# Development and validation

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

## Dependencies and references

Zolana is pinned to `0.1.6-alpha`. TVC is unpublished, and the published wallet-kit 1.1.0 lacks the TVC session APIs used here. The local `../vendor/` archives contain builds from these inspected revisions:

- [zolana-tvc](https://github.com/helius-labs/zolana-tvc/tree/35dafa8ad8afcbdb6099517f8b4f9db899a112a2)
- [wallet-kit browser integration](https://github.com/helius-labs/wallet-kit/tree/c00f7a5e5ee7a2e2013c8301c762f870a99f8ee7)

`../vendor/README.md`, `../vendor/SHA256SUMS`, and `../scripts/rebuild-vendor.sh` record provenance and rebuild commands. Only the React provider and signing/enrollment helpers are reused; the app does not adopt Next.js or the full demo UI.

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
- The migrated app type-checks, all 112 tests across 17 files pass, and its production build passes. Upstream WASM URL warnings and large bundle warnings remain.
- Live TVC attestation passed through the local proxy against the independently pinned policy, PCRs and Boot Proof.
- The real Turnkey email login dialog opens successfully with the configured Helius project. Wallet and login layouts were inspected at 390px and 1440px without horizontal overflow or page errors. Connected and activated layouts were inspected at both widths using isolated visual fixtures, not real transaction results.
- The production preview proxy returns the pinned release and rejects an untrusted Origin with HTTP 403.
- **Live transaction acceptance is partial.** The signed-in Turnkey wallet restored after reload, activation completed, and a user-submitted 0.001 SOL deposit appeared with a confirmed receipt and refreshed public/private balances. The live UI also showed a user-submitted self-transfer completing with stage timings, an explorer receipt, and refreshed balances. Withdrawal and transfer to a different registered recipient remain unverified. Automatic approval review requires the user to perform wallet activation and transaction clicks; the agent can inspect the resulting receipts and balances.

## Operation files

The React app imports registration, deposit, transfer, withdrawal, balance reads, and wallet sync from separate files in `src/operations/`. History reads and history sync are reusable helpers; the UI does not include a history feed. Query tests verify cached reads, sync-before-history ordering, indexer failure, and canceled sessions. The existing transaction, lifecycle, and progress tests run against the extracted operations.
