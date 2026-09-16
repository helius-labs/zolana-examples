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

Historical validation before the stateless refactor:

- The Privy checkpoint is commit `0207237`. Its message signing and onchain registration worked, but private sync failed with `WALLET_SYNC: CLIENT_REQUEST: API_REQUEST`; private actions were not verified.
- At the operation-extraction checkpoint, type checks, 112 tests across 17 files, and the production build passed. Upstream WASM URL warnings and large bundle warnings remain.
- Live TVC attestation passed through the local proxy against the independently pinned policy, PCRs and Boot Proof.
- The real Turnkey email login dialog opens successfully with the configured Helius project. Wallet and login layouts were inspected at 390px and 1440px without horizontal overflow or page errors. Connected and activated layouts were inspected at both widths using isolated visual fixtures, not real transaction results.
- The production preview proxy returns the pinned release and rejects an untrusted Origin with HTTP 403.
- **Live transaction acceptance is partial.** The signed-in Turnkey wallet restored after reload, activation completed, and a user-submitted 0.001 SOL deposit appeared with a confirmed receipt and refreshed public/private balances. The live UI also showed a user-submitted self-transfer completing with stage timings, an explorer receipt, and refreshed balances. Withdrawal and transfer to a different registered recipient remain unverified. Automatic approval review requires the user to perform wallet activation and transaction clicks; the agent can inspect the resulting receipts and balances.

## Stateless reads and instruction builders

The React context holds the active identity, TVC keys, client, and submission/cancellation functions. It does not hold an SDK `Wallet`. Each private read starts with all tag pages, decrypts into a request-local wallet, then queries discovered nullifiers until no unqueried spendable notes remain. Pagination cursors exist only within that request. SDK registry backfill resolves unknown assets; unresolved assets and unparsed transactions fail the read.

Deposit uses `getDepositInstructionAsync`. Transfer and withdrawal use `ConfidentialTransfer`, TVC transaction keys, `encryptConfidentialTransfer`, `client.proveTransact`, and `getTransactInstruction`. Solana Kit compiles v0 transactions; private spends retain the SDK default 300,000 compute-unit limit and packet-size checks. The plain-note selection policy matches the SDK: largest first, one tree, supported proof input count, no ring/data-bound notes. The connected owner remains the fee payer. No local SDK internals or hand-written cryptography are imported.

Operations are grouped under `src/operations/read/` (balance and history) and `src/operations/send/` (deposit, transfer, withdrawal, and registration), with their tests alongside them. The shared reader is the only local read/decrypt orchestration. Operation files call SDK builders directly rather than chaining cached-read/sync wrappers. History has one fresh-read helper and no UI feed. Full scans cost more than incremental sync. Account changes cancel requests, and the UI prevents concurrent actions; a conflicting spend from another session is rejected by the protocol and requires a fresh retry.

### Stateless-refactor validation (2026-09-12)

- Node 24.20.0: `pnpm check`, `pnpm test` (123 tests across 18 files), and `pnpm build` passed. Existing upstream WASM, Node-crypto browser externalization, and bundle-size warnings remain.
- Fresh read results match SDK `syncWallet` on shared fixtures, including encrypted transfers and notes spent elsewhere. Additional tests cover chained spends, pagination, duplicate events, old viewing-key tags, unknown assets, malformed history, service errors, and cancellation. `syncWallet` is used only as a test reference.
- Instruction tests exercise real SDK preparation/encryption and Solana Kit compilation with a stubbed prover. They verify chosen amounts, recipient/change outputs, SOL withdrawal settlement, packet construction, proof failure, temporary-key cleanup, and receipts surviving failed post-confirmation reads. Existing tests continue to check Turnkey signatures, registration, recovery, and duplicate UI operations.
- The local browser at `http://127.0.0.1:5173/` loads the Turnkey sign-in screen and the documentation/demo links. The session is signed out. **Live deposit, transfer, withdrawal, refreshed balances, and identity recovery after reload are pending sign-in with the existing Turnkey account.** No real devnet transactions were submitted during this refactor. Historical receipts above do not verify this implementation.
- Migration changes remain uncommitted for review. No merge or deployment was performed.


### Boot-proof origin blocker (2026-09-12)

Activation currently stops at `POST /api/tvc/boot-proof` with HTTP 403 and `{ "error": "CrossOriginRequestDenied" }`. This was reproduced through the local Vite proxy and directly against the configured shared backend, preserving `Origin: http://127.0.0.1:5173` and `X-Forwarded-Host: 127.0.0.1:5173`. The denial occurs before enrollment and private balance reads. The local proxy validates the browser origin and forwards the matching local host; the shared backend or its upstream proxy must support that origin. Do not rewrite Origin, disable attestation, or delete TVC recovery state to work around it. The UI now distinguishes this backend denial from a local proxy rejection.

### Public transfers and balance selection (2026-09-12)

- Select Private balance for Transfer or Withdraw; select Public balance for Transfer or Deposit. The shared form keeps separate public/private transfer amounts, clears recipients on source changes, and locks selection during submission. Existing orange controls, measured height transitions, keyboard focus, and reduced-motion support remain.
- `src/operations/send/publicTransfer.ts` builds `getTransferSolInstruction` from `@solana-program/system`, compiles through Solana Kit, and checks fresh public funds against the amount plus `getFeeForMessage`. The connected Turnkey session signs through the existing message/signature verification adapter. Public sends do not activate TVC, check private registration, read private history, or prove a private spend.
- Public transfer progress is Preparing → Sending → Confirmed. Confirmation and public refresh are separate: a failed refresh retains the receipt. Public transfer refresh reads only public SOL. Private read failures do not block public sends.
- Node 24.20.0: type checking, all 143 tests across 19 files, and the production build passed. Existing dependency WASM, Node-crypto browser externalization, eval, and bundle-size warnings remain. New tests cover real public instruction encoding, fee affordability, invalid inputs, cancellation, signing failures, duplicate submits, independent balance drafts, and confirmed receipts surviving refresh errors.
- Connected private/public forms and the public receipt were visually inspected at 390px and 1440px using isolated browser fixtures. Neither viewport had horizontal overflow or page errors; public progress omitted Proving. These fixtures do not establish live transaction success.
- The live browser is signed out. A real public devnet transfer remains unverified. The previously recorded TVC HTTP 403 remains a separate private-activation blocker; public transfers do not use that endpoint. No commit, merge, or deployment was performed.

### Total balance and deposit/withdraw recipients (2026-09-12)

- Added a non-selectable total of public and private SOL, using bigint arithmetic and the existing SOL formatter. Unknown balances produce an unknown total, rather than a partial sum presented as the total.
- Both balance views contain Deposit, Private Transfer or Public Transfer, and Withdraw. Deposit always spends public SOL and withdrawal always spends private SOL. Switching views preserves the selected action and resets recipient choices; the funding-source label and amount validation follow the actual operation.
- Deposit and withdrawal default to My wallet and offer Another wallet. External deposits resolve the registered shielded identity before building; withdrawals bind the selected public recipient into preparation, intent verification, and the instruction. The connected wallet remains the payer and receives private change. Invalid addresses, unregistered deposit recipients, and stale account results fail before submission.
- Recipient details expand through the existing 340ms height animation. The three-action highlight moves and resizes over 480ms. Total balance changes also animate, and reduced-motion settings are preserved. The wider middle segment keeps Private Transfer and Public Transfer on one line.
- Node 24: type checking, 159 tests across 19 files, and the production build passed. Existing upstream dependency warnings remain. Connected layouts and recipient choices were inspected using isolated fixtures at 320px, 390px, and 1440px. These checks do not establish live third-party deposit or withdrawal success; no live transactions were sent for this change.

### Origin rejection recheck (2026-09-12)

A fresh POST through `http://127.0.0.1:5173/api/tvc/boot-proof` returned HTTP 403 with `{"error":"CrossOriginRequestDenied"}`. The diagnostic body was deliberately invalid (`{"ephemeralKey":"diagnostic-invalid-key"}`), contained no user credentials, and could not complete a boot proof or enrollment.

The local proxy validates the incoming Origin against Host, preserves Origin, and explicitly sends `X-Forwarded-Host: 127.0.0.1:5173`. The reference `sameRequestOrigin()` at wallet-kit revision `c00f7a5e5ee7a2e2013c8301c762f870a99f8ee7` accepts an Origin whose host matches that forwarded host. Its boot-proof catch returns HTTP 502 with `TvcProxyFailed`, not the live HTTP 403/body. The `feature/tvc-four-operations` revision `fe512c2b0edcfb94779700318fca6a6e49dc36e3` has the same origin logic and generic error response. Therefore the live service has different deployed behavior or an additional upstream guard; these reference files alone do not identify the rejecting component.

The next required evidence is the deployed application revision and the Origin/Host/X-Forwarded-Host values received by its origin guard. Correct the trusted proxy forwarding or the explicit origin configuration there, then repeat the invalid-body probe: it should reach body validation, without issuing a proof. Real activation must then pass attestation and enrollment normally. Do not change the app's Origin header, relax attestation, or replace the backend to hide the rejection. No backend deployment was attempted.
