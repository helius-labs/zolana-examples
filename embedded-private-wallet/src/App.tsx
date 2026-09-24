import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
} from "react";
import { useEmbeddedWallet } from "./hooks/useEmbeddedWallet";
import { address } from "@solana/kit";
import { usePrivateWallet } from "./hooks/usePrivateWallet";
import { BalanceSyncError } from "./lib/syncAfterTransaction";
import {
  DEPOSIT_AMOUNT,
  TRANSFER_AMOUNT,
  WITHDRAW_AMOUNT,
} from "./lib/amounts";
import { depositSol } from "./operations/send/deposit";
import { transferSol } from "./operations/send/transfer";
import { transferPublicSol } from "./operations/send/publicTransfer";
import { createPublicWalletContext } from "./lib/publicWalletContext";
import { withdrawSol } from "./operations/send/withdraw";
import {
  getPublicSolBalance,
  getPrivateSolBalance,
} from "./operations/read/getBalance";
import { parseSol } from "./lib/parseSol";
import { walletError } from "./lib/walletError";
import { formatSol } from "./lib/formatSol";
import { withTimeout } from "./lib/withTimeout";
import { TransferStepper } from "./TransferStepper";
import { MotionRegion } from "./MotionRegion";
import type {
  TransferProgress,
  TransferProgressCallback,
} from "./lib/transferProgress";

const progress = {
  initializing: "Preparing private wallet…",
  verifying: "Verifying private wallet service…",
  enrolling: "Connecting your Turnkey wallet…",
  signing: "Activating private wallet…",
  registering: "Registering private wallet…",
  syncing: "Syncing balance…",
};
const amounts = {
  Deposit: DEPOSIT_AMOUNT,
  Transfer: TRANSFER_AMOUNT,
  Withdraw: WITHDRAW_AMOUNT,
};
type Action = keyof typeof amounts;
const shorten = (value: string) => `${value.slice(0, 6)}…${value.slice(-6)}`;
const errorText = (error: unknown) =>
  error instanceof Error ? error.message : String(error);

export default function App() {
  const { ready, authenticated, connected, sessionKey, login, logout, clear } =
    useEmbeddedWallet();
  const [creating, setCreating] = useState(false);
  const [loginError, setLoginError] = useState<string | null>(null);
  const creationInFlight = useRef(false);
  const loginSession = useRef(sessionKey);
  useLayoutEffect(() => {
    loginSession.current = sessionKey;
    return () => {
      loginSession.current = "";
    };
  }, [sessionKey]);
  async function createEmbeddedWallet() {
    if (creationInFlight.current) return;
    creationInFlight.current = true;
    setCreating(true);
    setLoginError(null);
    const current = sessionKey;
    try {
      if (authenticated) await clear();
      await login();
    } catch (error) {
      if (loginSession.current === current) setLoginError(errorText(error));
    } finally {
      creationInFlight.current = false;
      setCreating(false);
    }
  }
  return (
    <main className="wallet-page">
      <div className="wallet-shell">
        <header className="page-header">
          <h1>Private wallet</h1>
          <span className="network">Devnet</span>
        </header>
        <MotionRegion transitionKey={connected} className="wallet-panel-frame">
          {connected ? (
            <ConnectedWallet key={sessionKey} />
          ) : (
            <section className="wallet-panel empty-wallet" aria-label="Sign in">
              <svg
                className="wallet-symbol"
                width="40"
                height="40"
                viewBox="0 0 40 40"
                fill="none"
                aria-hidden="true"
              >
                <rect
                  x="5"
                  y="9"
                  width="30"
                  height="25"
                  rx="5"
                  stroke="currentColor"
                  strokeWidth="1.5"
                />
                <path
                  d="M5 14V10a4 4 0 0 1 3-4l20-3v6M35 19h-8a3 3 0 0 0 0 6h8"
                  stroke="currentColor"
                  strokeWidth="1.5"
                />
                <circle cx="28" cy="22" r="1" fill="currentColor" />
              </svg>
              <h2>
                {authenticated ? "Your embedded wallet" : "Your private wallet"}
              </h2>
              <p>
                Sign in with Turnkey to view your balances and send public or
                private SOL.
              </p>
              <button
                className="primary-button"
                disabled={!ready || creating}
                onClick={() => void createEmbeddedWallet()}
              >
                {!ready
                  ? "Loading wallet…"
                  : creating
                    ? "Opening wallet…"
                    : authenticated
                      ? "Sign in again"
                      : "Sign in with Turnkey"}
              </button>
              {authenticated && (
                <button className="text-button" onClick={() => void logout()}>
                  Sign out
                </button>
              )}
              <MotionRegion>
                {loginError && (
                  <p className="error-message" role="alert">
                    {loginError}
                  </p>
                )}
              </MotionRegion>
            </section>
          )}
        </MotionRegion>
        <footer className="wallet-footer">
          <a
            href="https://www.helius.dev/docs/privacy"
            target="_blank"
            rel="noreferrer"
          >
            Documentation
          </a>
          <span aria-hidden="true">–</span>
          <a
            href="https://helius.dev/privacy/demo"
            target="_blank"
            rel="noreferrer"
          >
            Launch Demo
          </a>
        </footer>
      </div>
    </main>
  );
}

function ConnectedWallet() {
  const { sessionKey, logout, signTransaction } = useEmbeddedWallet();
  const { ready, status, error, ctx, owner, initialize } = usePrivateWallet();
  const [source, setSource] = useState<"private" | "public">("private");
  const [action, setAction] = useState<Action>("Transfer");
  const [publicAmount, setPublicAmount] = useState(formatSol(TRANSFER_AMOUNT));
  const [amountInputs, setAmountInputs] = useState<Record<Action, string>>({
    Deposit: formatSol(DEPOSIT_AMOUNT),
    Transfer: formatSol(TRANSFER_AMOUNT),
    Withdraw: formatSol(WITHDRAW_AMOUNT),
  });
  const [recipient, setRecipient] = useState("");
  const [recipientMode, setRecipientMode] = useState<"own" | "other">("own");
  const [signature, setSignature] = useState<string | null>(null);
  const [txError, setTxError] = useState<string | null>(null);
  const [pending, setPending] = useState(false);
  const [transferProgress, setTransferProgress] =
    useState<TransferProgress | null>(null);
  const [publicBalance, setPublicBalance] = useState<bigint | null>(null);
  const [privateBalance, setPrivateBalance] = useState<bigint | null>(null);
  const [publicBalanceError, setPublicBalanceError] = useState<string | null>(
    null,
  );
  const [privateBalanceError, setPrivateBalanceError] = useState<string | null>(
    null,
  );
  const balanceError = [publicBalanceError, privateBalanceError]
    .filter(Boolean)
    .join(" ");
  const [refreshing, setRefreshing] = useState(false);
  const [copyStatus, setCopyStatus] = useState("");
  const session = useRef(0);
  const operation = useRef(false);
  const balanceRequest = useRef(0);
  const balanceAbort = useRef<AbortController | null>(null);
  const actionAbort = useRef<AbortController | null>(null);
  const [publicRefreshing, setPublicRefreshing] = useState(false);
  const initializing = status in progress;
  const publicTransfer = source === "public" && action === "Transfer";
  const amountInput = publicTransfer ? publicAmount : amountInputs[action];
  const busyReading = publicTransfer ? publicRefreshing : refreshing;
  const blockedBalance = publicTransfer
    ? publicBalance === null
    : Boolean(balanceError) ||
      privateBalance === null ||
      publicBalance === null;
  const needsActivation = !ready && !publicTransfer;
  const actions: Action[] = ["Deposit", "Transfer", "Withdraw"];
  const fundingSource =
    action === "Deposit"
      ? "public"
      : action === "Withdraw"
        ? "private"
        : source;
  const customRecipient = action === "Transfer" || recipientMode === "other";
  const totalBalance =
    publicBalance === null || privateBalance === null
      ? null
      : publicBalance + privateBalance;
  let amount: bigint | null = null;
  let amountError: string | null = null;
  try {
    amount = parseSol(amountInput);
    const available =
      fundingSource === "public" ? publicBalance : privateBalance;
    if (available !== null && amount > available) {
      amountError = `Amount exceeds your ${fundingSource} SOL balance.`;
    }
  } catch (error) {
    amountError = errorText(error);
  }

  useLayoutEffect(() => {
    session.current += 1;
    operation.current = false;
    balanceRequest.current += 1;
    setSignature(null);
    setTxError(null);
    setPublicBalance(null);
    setPrivateBalance(null);
    setPublicBalanceError(null);
    setPrivateBalanceError(null);
    setCopyStatus("");
    setRecipient("");
    setRecipientMode("own");
    setSource("private");
    setAction("Transfer");
    setPublicAmount(formatSol(TRANSFER_AMOUNT));
    setAmountInputs({
      Deposit: formatSol(DEPOSIT_AMOUNT),
      Transfer: formatSol(TRANSFER_AMOUNT),
      Withdraw: formatSol(WITHDRAW_AMOUNT),
    });
    setPending(false);
    setTransferProgress(null);
    setRefreshing(false);
    setPublicRefreshing(false);
    return () => {
      session.current += 1;
      balanceRequest.current += 1;
      balanceAbort.current?.abort();
      actionAbort.current?.abort();
    };
  }, [owner, sessionKey]);

  const loadBalances = useCallback(
    async (knownPrivateBalance?: bigint) => {
      if (!owner) return false;
      const currentSession = session.current;
      const request = ++balanceRequest.current;
      balanceAbort.current?.abort();
      const controller = new AbortController();
      balanceAbort.current = controller;
      const active = () =>
        session.current === currentSession &&
        balanceRequest.current === request;
      setRefreshing(true);
      setPublicRefreshing(true);
      setPublicBalanceError(null);
      setPrivateBalanceError(null);
      const results = await Promise.allSettled([
        withTimeout(
          getPublicSolBalance(
            owner,
            ctx?.client,
            AbortSignal.any([controller.signal, AbortSignal.timeout(15_000)]),
          ),
        )
          .then((balance) => {
            if (active()) setPublicBalance(balance);
            return balance;
          })
          .finally(() => {
            if (active()) setPublicRefreshing(false);
          }),
        withTimeout(
          (async () => {
            if (!ctx) return null;
            if (knownPrivateBalance !== undefined) return knownPrivateBalance;
            return getPrivateSolBalance(ctx, {
              signal: AbortSignal.any([
                controller.signal,
                AbortSignal.timeout(60_000),
              ]),
            });
          })(),
          60_000,
        ).then((balance) => {
          if (active()) setPrivateBalance(balance);
          return balance;
        }),
      ]);
      if (!active()) return false;
      setPublicBalance(
        results[0].status === "fulfilled" ? results[0].value : null,
      );
      setPrivateBalance(
        results[1].status === "fulfilled" ? results[1].value : null,
      );
      const failed = results.some((result) => result.status === "rejected");
      setPublicBalanceError(
        results[0].status === "rejected"
          ? "Couldn’t refresh public SOL. Check the RPC connection and try again."
          : null,
      );
      setPrivateBalanceError(
        results[1].status === "rejected"
          ? "Couldn’t sync private SOL. Try refreshing again."
          : null,
      );
      setRefreshing(false);
      return !failed;
    },
    [owner, ctx],
  );

  useEffect(() => {
    void loadBalances();
  }, [loadBalances]);

  async function refresh() {
    if (operation.current || initializing || busyReading) return;
    operation.current = true;
    const currentSession = session.current;
    try {
      if (publicTransfer) {
        const controller = new AbortController();
        const request = ++balanceRequest.current;
        balanceAbort.current?.abort();
        balanceAbort.current = controller;
        setRefreshing(true);
        setPublicRefreshing(true);
        const active = () =>
          session.current === currentSession &&
          balanceRequest.current === request;
        try {
          const balance = await withTimeout(
            getPublicSolBalance(
              owner,
              ctx?.client,
              AbortSignal.any([controller.signal, AbortSignal.timeout(15_000)]),
            ),
          );
          if (active()) {
            setPublicBalance(balance);
            setPublicBalanceError(null);
            setTxError(null);
            setTransferProgress(null);
          }
        } catch {
          if (active()) {
            setPublicBalance(null);
            setPublicBalanceError(
              "Couldn’t refresh public SOL. Check the RPC connection and try again.",
            );
          }
        } finally {
          if (active()) {
            setRefreshing(false);
            setPublicRefreshing(false);
          }
        }
        return;
      }
      const refreshed = await loadBalances();
      if (refreshed && session.current === currentSession && signature) {
        setTxError(null);
        setTransferProgress(null);
      }
    } finally {
      if (session.current === currentSession) operation.current = false;
    }
  }

  async function run() {
    if (
      (!publicTransfer && (!ready || !ctx)) ||
      operation.current ||
      busyReading ||
      blockedBalance
    )
      return;
    operation.current = true;
    const currentSession = session.current;
    const controller = new AbortController();
    actionAbort.current = controller;
    const assertActive = () => {
      controller.signal.throwIfAborted();
      if (session.current !== currentSession)
        throw new Error("Wallet session changed.");
    };
    if (publicTransfer) {
      balanceRequest.current += 1;
      balanceAbort.current?.abort();
      setRefreshing(false);
      setPublicRefreshing(false);
    }
    setPending(true);
    setTxError(null);
    setSignature(null);
    setTransferProgress(null);
    const reportProgress: TransferProgressCallback = (
      stage,
      confirmedSignature,
    ) => {
      if (session.current !== currentSession) return;
      const at = performance.now();
      setTransferProgress((previous) => ({
        stage,
        failed: false,
        marks:
          previous?.stage === stage
            ? previous.marks
            : [...(previous?.marks ?? []), { stage, at }],
      }));
      if (confirmedSignature) setSignature(confirmedSignature);
    };
    try {
      const selectedAmount = parseSol(amountInput);
      const available =
        fundingSource === "public" ? publicBalance : privateBalance;
      if (available !== null && selectedAmount > available)
        throw new Error(`Amount exceeds your ${fundingSource} SOL balance.`);
      let destination;
      try {
        destination = address(customRecipient ? recipient.trim() : owner);
      } catch {
        throw new Error("Enter a valid Solana recipient address.");
      }
      let result: { signature: string; privateBalance: bigint };
      if (action === "Transfer") {
        reportProgress("preparing");
        if (publicTransfer) {
          const publicCtx = await createPublicWalletContext(
            owner,
            signTransaction,
            controller.signal,
            assertActive,
          );
          const confirmed = await transferPublicSol(
            publicCtx,
            destination,
            selectedAmount,
            reportProgress,
          );
          assertActive();
          setSignature(confirmed.signature);
          try {
            const balance = await withTimeout(
              getPublicSolBalance(
                owner,
                publicCtx.client,
                AbortSignal.any([
                  controller.signal,
                  AbortSignal.timeout(15_000),
                ]),
              ),
            );
            assertActive();
            setPublicBalance(balance);
            reportProgress("done");
          } catch (error) {
            assertActive();
            setPublicBalance(null);
            throw new Error(
              "Transfer confirmed, but public balance could not refresh. Refresh balances to try again.",
            );
          }
          return;
        }
        result = await transferSol(
          ctx!,
          destination,
          selectedAmount,
          reportProgress,
        );
      } else {
        result = await (action === "Deposit"
          ? depositSol(ctx!, selectedAmount, destination)
          : withdrawSol(ctx!, selectedAmount, destination));
      }
      if (session.current !== currentSession) return;
      setSignature(result.signature);
      await loadBalances(result.privateBalance);
    } catch (e: unknown) {
      if (session.current === currentSession) {
        if (e instanceof BalanceSyncError) {
          setSignature(e.signature);
          setPrivateBalance(null);
          const request = ++balanceRequest.current;
          balanceAbort.current?.abort();
          const controller = new AbortController();
          balanceAbort.current = controller;
          const active = () =>
            session.current === currentSession &&
            balanceRequest.current === request;
          void getPublicSolBalance(
            owner,
            ctx?.client,
            AbortSignal.any([controller.signal, AbortSignal.timeout(15_000)]),
          )
            .then((balance) => {
              if (active()) setPublicBalance(balance);
            })
            .catch(() => {
              if (active()) setPublicBalance(null);
            });
          setPrivateBalanceError(
            "Refresh balances before making another transaction.",
          );
        }
        setTxError(walletError(e));
        setTransferProgress((previous) =>
          previous ? { ...previous, failed: true } : null,
        );
      }
    } finally {
      if (session.current === currentSession) {
        operation.current = false;
        setPending(false);
      }
    }
  }

  async function copyAddress() {
    const currentSession = session.current;
    try {
      await navigator.clipboard.writeText(owner);
      if (session.current === currentSession) setCopyStatus("Address copied");
    } catch {
      if (session.current === currentSession)
        setCopyStatus("Couldn’t copy. Select the address to copy it.");
    }
  }

  return (
    <section className="wallet-panel" aria-label="Wallet">
      <div className="account-row">
        <div className="account-info">
          <span className="eyebrow">Connected wallet</span>
          <span className="wallet-name">Turnkey embedded wallet</span>
        </div>
        <button className="text-button" onClick={() => void logout()}>
          Sign out
        </button>
      </div>
      <MotionRegion>
        <div className="address-row">
          <details className="address-details">
            <summary title={owner}>{shorten(owner)}</summary>
            <span className="full-address">{owner}</span>
          </details>
          <button
            className="text-button"
            onClick={() => void copyAddress()}
            aria-label="Copy wallet address"
          >
            Copy
          </button>
        </div>
        <p className="sr-only" role="status">
          {copyStatus}
        </p>
        {copyStatus && <p className="copy-feedback">{copyStatus}</p>}
      </MotionRegion>

      <div className="balance-section">
        <MotionRegion className="total-balance">
          <span className="eyebrow">Total balance</span>
          <p className="balance" aria-label="Total SOL balance">
            <span
              className="balance-value"
              key={totalBalance?.toString() ?? "loading"}
            >
              {totalBalance === null ? "—" : formatSol(totalBalance)}
            </span>{" "}
            <span className="balance-unit">SOL</span>
          </p>
          <p className="help-text">
            {totalBalance === null
              ? "Total appears when both balances are available."
              : "Public and private SOL"}
          </p>
        </MotionRegion>
        <fieldset className="balance-picker" disabled={pending || initializing}>
          <legend className="sr-only">Balance to use</legend>
          {(["private", "public"] as const).map((option) => {
            const balance =
              option === "private" ? privateBalance : publicBalance;
            const label = option === "private" ? "Private" : "Public";
            return (
              <label className="balance-option" key={option}>
                <input
                  type="radio"
                  name="balance-source"
                  value={option}
                  aria-label={`${label} balance`}
                  checked={source === option}
                  onChange={() => {
                    if (operation.current) return;
                    setSource(option);
                    setRecipient("");
                    setRecipientMode("own");
                    setSignature(null);
                    setTxError(null);
                    setTransferProgress(null);
                  }}
                />
                <span className="balance-card">
                  <span className="balance-card-label">
                    {label} balance
                    <span className="selection-dot" aria-hidden="true" />
                  </span>
                  <span
                    className="balance-card-value"
                    aria-label={`${label} SOL balance`}
                  >
                    {balance === null ? "—" : formatSol(balance)}{" "}
                    <span className="balance-unit">SOL</span>
                  </span>
                </span>
              </label>
            );
          })}
        </fieldset>
        <div className="refresh-row">
          <button
            className="text-button"
            disabled={pending || initializing || busyReading}
            onClick={() => void refresh()}
          >
            {busyReading ? "Refreshing…" : "Refresh balances"}
          </button>
        </div>
        <MotionRegion>
          {balanceError && (
            <p className="error-message" role="alert">
              {balanceError}
            </p>
          )}
        </MotionRegion>
      </div>

      <div className="action-section">
        <fieldset
          className="action-picker"
          data-action={action}
          disabled={pending || busyReading || initializing}
        >
          <legend className="sr-only">Action</legend>
          {actions.map((option) => (
            <label key={option}>
              <input
                type="radio"
                name="action"
                value={option}
                checked={action === option}
                onChange={() => {
                  setAction(option);
                  setRecipient("");
                  setRecipientMode("own");
                  setTxError(null);
                  setTransferProgress(null);
                  setSignature(null);
                }}
              />
              <span>
                {option === "Transfer"
                  ? source === "private"
                    ? "Private Transfer"
                    : "Public Transfer"
                  : option}
              </span>
            </label>
          ))}
        </fieldset>
        <MotionRegion transitionKey={`${source}:${action}:${needsActivation}`}>
          {needsActivation ? (
            <div className="enable-section">
              <h2>Activate your private wallet</h2>
              <p className="help-text">
                Activate your private wallet with Turnkey. First-time setup
                registers your wallet address onchain so others can send you
                private transfers. Learn more in the{" "}
                <a
                  href="https://www.helius.dev/docs/privacy/concepts"
                  target="_blank"
                  rel="noreferrer"
                >
                  Docs
                </a>
                .
              </p>
              <button
                className="primary-button"
                disabled={initializing}
                onClick={() => void initialize()}
              >
                {initializing ? (
                  <>
                    <span className="spinner" aria-hidden="true" />
                    Activating…
                  </>
                ) : error ? (
                  "Try again"
                ) : (
                  "Activate private wallet"
                )}
              </button>
              <MotionRegion>
                <p className="status-message" role="status">
                  {initializing
                    ? progress[status as keyof typeof progress]
                    : ""}
                </p>
                {error && (
                  <p className="error-message" role="alert">
                    {error}
                  </p>
                )}
              </MotionRegion>
            </div>
          ) : (
            <form
              className="transaction-form"
              onSubmit={(event) => {
                event.preventDefault();
                void run();
              }}
            >
              <h2>
                {action === "Transfer"
                  ? publicTransfer
                    ? "Public transfer"
                    : "Private transfer"
                  : action === "Deposit"
                    ? "Deposit to private balance"
                    : "Withdraw to public balance"}
              </h2>
              <div className="amount-row">
                <label htmlFor="amount">Amount</label>
                <div className="amount-input">
                  <input
                    id="amount"
                    type="text"
                    inputMode="decimal"
                    autoComplete="off"
                    spellCheck={false}
                    value={amountInput}
                    onChange={(event) => {
                      if (publicTransfer) setPublicAmount(event.target.value);
                      else
                        setAmountInputs((current) => ({
                          ...current,
                          [action]: event.target.value,
                        }));
                      setTxError(null);
                    }}
                    disabled={pending || busyReading}
                    aria-invalid={Boolean(amountError)}
                    aria-describedby={amountError ? "amount-error" : undefined}
                  />
                  <span>SOL</span>
                </div>
              </div>
              <MotionRegion>
                {amountError && (
                  <p id="amount-error" className="error-message" role="status">
                    {amountError}
                  </p>
                )}
              </MotionRegion>
              <div className="action-content" key={action}>
                <p className="destination">
                  {action === "Deposit"
                    ? "From your public balance to a private wallet."
                    : action === "Withdraw"
                      ? "From your private balance to a public wallet."
                      : publicTransfer
                        ? "From your public balance to a public wallet."
                        : "From your private balance to a private wallet."}
                </p>
                {action !== "Transfer" && (
                  <fieldset
                    className="recipient-picker"
                    disabled={pending || busyReading}
                  >
                    <legend>Recipient</legend>
                    {(["own", "other"] as const).map((mode) => (
                      <label key={mode}>
                        <input
                          type="radio"
                          name="recipient-mode"
                          value={mode}
                          checked={recipientMode === mode}
                          onChange={() => {
                            setRecipientMode(mode);
                            setRecipient("");
                            setTxError(null);
                          }}
                        />
                        <span>
                          {mode === "own" ? "My wallet" : "Another wallet"}
                        </span>
                      </label>
                    ))}
                  </fieldset>
                )}
                <MotionRegion transitionKey={customRecipient}>
                  {customRecipient ? (
                    <div className="recipient-field">
                      <label htmlFor="recipient">
                        {action === "Transfer"
                          ? "Recipient"
                          : "Recipient address"}
                      </label>
                      <input
                        id="recipient"
                        value={recipient}
                        onChange={(event) => {
                          setRecipient(event.target.value);
                          setTxError(null);
                        }}
                        placeholder="Solana address"
                        autoComplete="off"
                        autoCapitalize="none"
                        spellCheck={false}
                        disabled={pending || busyReading}
                        aria-describedby="recipient-help"
                      />
                      <p id="recipient-help" className="help-text">
                        {action === "Withdraw" || publicTransfer
                          ? "Enter the recipient’s Solana address."
                          : "Use an address with a registered private wallet."}
                      </p>
                    </div>
                  ) : (
                    <div className="own-recipient">
                      <span className="help-text">Your connected wallet</span>
                      <span className="recipient-address" title={owner}>
                        {shorten(owner)}
                      </span>
                    </div>
                  )}
                </MotionRegion>
              </div>
              <button
                className="primary-button"
                type="submit"
                disabled={
                  pending ||
                  busyReading ||
                  blockedBalance ||
                  Boolean(amountError) ||
                  (customRecipient && !recipient.trim())
                }
              >
                {pending ? (
                  <>
                    <span className="spinner" aria-hidden="true" />
                    {action === "Deposit"
                      ? "Depositing…"
                      : action === "Transfer"
                        ? "Transferring…"
                        : "Withdrawing…"}
                  </>
                ) : amount !== null && !amountError ? (
                  `${action} ${formatSol(amount)} SOL`
                ) : (
                  action
                )}
              </button>
              <MotionRegion>
                {transferProgress && (
                  <TransferStepper
                    progress={transferProgress}
                    kind={publicTransfer ? "public" : "private"}
                  />
                )}
                <div className="transaction-result" role="status">
                  {pending && !transferProgress && (
                    <p>Approve in your wallet, then wait for confirmation.</p>
                  )}
                  {signature && (
                    <>
                      <a
                        href={`https://explorer.solana.com/tx/${signature}?cluster=devnet`}
                        target="_blank"
                        rel="noreferrer"
                        title={signature}
                      >
                        View transaction{" "}
                        <span className="signature">{shorten(signature)}</span>
                        <span aria-hidden="true"> ↗</span>
                      </a>
                    </>
                  )}
                </div>
                {txError && (
                  <p className="error-message" role="alert">
                    {txError}
                  </p>
                )}
              </MotionRegion>
            </form>
          )}
        </MotionRegion>
      </div>
    </section>
  );
}
