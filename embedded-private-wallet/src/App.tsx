import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
} from "react";
import { useEmbeddedWallet } from "./hooks/useEmbeddedWallet";
import { useRpcConnection } from "./hooks/useRpcConnection";
import { address } from "@solana/kit";
import { usePrivateWallet } from "./hooks/usePrivateWallet";
import { BalanceSyncError } from "./lib/syncAfterTransaction";
import {
  DEPOSIT_AMOUNT,
  TRANSFER_AMOUNT,
  WITHDRAW_AMOUNT,
} from "./lib/amounts";
import { depositSol } from "./operations/deposit";
import { transferSol } from "./operations/transfer";
import { withdrawSol } from "./operations/withdraw";
import {
  getPublicSolBalance,
  getPrivateSolBalance,
} from "./operations/getBalance";
import { syncPrivateWallet } from "./operations/syncWallet";
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
                Sign in with Turnkey to view your balance and test private
                transfers.
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
  const { sessionKey, logout } = useEmbeddedWallet();
  const connection = useRpcConnection();
  const { ready, status, error, ctx, owner, initialize } = usePrivateWallet();
  const [action, setAction] = useState<Action>("Deposit");
  const [amountInputs, setAmountInputs] = useState<Record<Action, string>>({
    Deposit: formatSol(DEPOSIT_AMOUNT),
    Transfer: formatSol(TRANSFER_AMOUNT),
    Withdraw: formatSol(WITHDRAW_AMOUNT),
  });
  const [recipient, setRecipient] = useState("");
  const [signature, setSignature] = useState<string | null>(null);
  const [txError, setTxError] = useState<string | null>(null);
  const [pending, setPending] = useState(false);
  const [transferProgress, setTransferProgress] =
    useState<TransferProgress | null>(null);
  const [publicBalance, setPublicBalance] = useState<bigint | null>(null);
  const [privateBalance, setPrivateBalance] = useState<bigint | null>(null);
  const [balanceError, setBalanceError] = useState<string | null>(null);
  const [refreshing, setRefreshing] = useState(false);
  const [copyStatus, setCopyStatus] = useState("");
  const session = useRef(0);
  const operation = useRef(false);
  const balanceRequest = useRef(0);
  const initializing = status in progress;
  let amount: bigint | null = null;
  let amountError: string | null = null;
  try {
    amount = parseSol(amountInputs[action]);
    const available = action === "Deposit" ? publicBalance : privateBalance;
    if (available !== null && amount > available) {
      amountError = `Amount exceeds your ${
        action === "Deposit" ? "public" : "private"
      } SOL balance.`;
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
    setBalanceError(null);
    setCopyStatus("");
    setRecipient("");
    setAmountInputs({
      Deposit: formatSol(DEPOSIT_AMOUNT),
      Transfer: formatSol(TRANSFER_AMOUNT),
      Withdraw: formatSol(WITHDRAW_AMOUNT),
    });
    setPending(false);
    setTransferProgress(null);
    setRefreshing(false);
    return () => {
      session.current += 1;
      balanceRequest.current += 1;
    };
  }, [owner, sessionKey]);

  const loadBalances = useCallback(
    async (syncPrivate = false) => {
      if (!owner) return false;
      const currentSession = session.current;
      const request = ++balanceRequest.current;
      const active = () =>
        session.current === currentSession &&
        balanceRequest.current === request;
      setRefreshing(true);
      setBalanceError(null);
      const results = await Promise.allSettled([
        withTimeout(getPublicSolBalance(connection, owner)).then((balance) => {
          if (active()) setPublicBalance(balance);
          return balance;
        }),
        withTimeout(
          (async () => {
            if (!ctx) return null;
            if (syncPrivate)
              await syncPrivateWallet(
                ctx,
                undefined,
                AbortSignal.timeout(15_000)
              );
            return getPrivateSolBalance(ctx);
          })()
        ).then((balance) => {
          if (active()) setPrivateBalance(balance);
          return balance;
        }),
      ]);
      if (!active()) return false;
      setPublicBalance(
        results[0].status === "fulfilled" ? results[0].value : null
      );
      setPrivateBalance(
        results[1].status === "fulfilled" ? results[1].value : null
      );
      const failed = results.some((result) => result.status === "rejected");
      if (failed) {
        setBalanceError(
          [
            results[0].status === "rejected"
              ? "Couldn’t refresh public SOL. Check the RPC connection and try again."
              : "",
            results[1].status === "rejected"
              ? "Couldn’t sync private SOL. Try refreshing again."
              : "",
          ]
            .filter(Boolean)
            .join(" ")
        );
      }
      setRefreshing(false);
      return !failed;
    },
    [connection, owner, ctx]
  );

  useEffect(() => {
    void loadBalances();
  }, [loadBalances]);

  async function refresh() {
    if (operation.current || initializing || refreshing) return;
    operation.current = true;
    const currentSession = session.current;
    try {
      const refreshed = await loadBalances(true);
      if (refreshed && session.current === currentSession && signature) {
        setTxError(null);
        setTransferProgress(null);
      }
    } finally {
      if (session.current === currentSession) operation.current = false;
    }
  }

  async function run() {
    if (!ready || !ctx || operation.current || refreshing || balanceError)
      return;
    operation.current = true;
    const currentSession = session.current;
    setPending(true);
    setTxError(null);
    setSignature(null);
    setTransferProgress(null);
    const reportProgress: TransferProgressCallback = (
      stage,
      confirmedSignature
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
      const selectedAmount = parseSol(amountInputs[action]);
      const available = action === "Deposit" ? publicBalance : privateBalance;
      if (available !== null && selectedAmount > available)
        throw new Error(
          `Amount exceeds your ${
            action === "Deposit" ? "public" : "private"
          } SOL balance.`
        );
      let result: { signature: string };
      if (action === "Transfer") {
        let destination;
        try {
          destination = address(recipient.trim());
        } catch {
          throw new Error("Enter a valid Solana recipient address.");
        }
        reportProgress("preparing");
        result = await transferSol(
          ctx,
          destination,
          selectedAmount,
          reportProgress
        );
      } else {
        result = await (action === "Deposit"
          ? depositSol(ctx, selectedAmount)
          : withdrawSol(ctx, selectedAmount));
      }
      if (session.current !== currentSession) return;
      setSignature(result.signature);
      await loadBalances();
    } catch (e: unknown) {
      if (session.current === currentSession) {
        if (e instanceof BalanceSyncError) {
          setSignature(e.signature);
          setPrivateBalance(null);
          setBalanceError(
            "Refresh balances before making another transaction."
          );
        }
        setTxError(walletError(e));
        setTransferProgress((previous) =>
          previous ? { ...previous, failed: true } : null
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
        <span className="eyebrow">Private balance</span>
        <MotionRegion>
          <p className="balance" aria-label="Private SOL balance">
            <span
              className="balance-value"
              key={privateBalance?.toString() ?? "loading"}
            >
              {privateBalance === null ? "—" : formatSol(privateBalance)}
            </span>{" "}
            <span className="balance-unit">SOL</span>
          </p>
        </MotionRegion>
        <MotionRegion>
          <div className="public-balance-row">
            <span>Public balance</span>
            <span
              className="numeric balance-value"
              key={publicBalance?.toString() ?? "loading"}
            >
              {publicBalance === null ? "—" : formatSol(publicBalance)} SOL
            </span>
          </div>
        </MotionRegion>
        <div className="refresh-row">
          <button
            className="text-button"
            disabled={pending || initializing || refreshing}
            onClick={() => void refresh()}
          >
            {refreshing ? "Refreshing…" : "Refresh balances"}
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

      <MotionRegion transitionKey={ready}>
        {!ready ? (
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
                {initializing ? progress[status as keyof typeof progress] : ""}
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
            <fieldset
              className="action-picker"
              data-action={action}
              disabled={pending || refreshing}
            >
              <legend className="sr-only">Action</legend>
              {(Object.keys(amounts) as Action[]).map((option) => (
                <label key={option}>
                  <input
                    type="radio"
                    name="action"
                    value={option}
                    checked={action === option}
                    onChange={() => {
                      setAction(option);
                      setTxError(null);
                      setTransferProgress(null);
                      setSignature(null);
                    }}
                  />
                  <span>{option}</span>
                </label>
              ))}
            </fieldset>
            <div className="amount-row">
              <label htmlFor="amount">Amount</label>
              <div className="amount-input">
                <input
                  id="amount"
                  type="text"
                  inputMode="decimal"
                  autoComplete="off"
                  spellCheck={false}
                  value={amountInputs[action]}
                  onChange={(event) => {
                    setAmountInputs((current) => ({
                      ...current,
                      [action]: event.target.value,
                    }));
                    setTxError(null);
                  }}
                  disabled={pending || refreshing}
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
            <MotionRegion>
              <div className="action-content" key={action}>
                {action === "Transfer" ? (
                  <div className="recipient-field">
                    <label htmlFor="recipient">Recipient</label>
                    <input
                      id="recipient"
                      value={recipient}
                      onChange={(event) => setRecipient(event.target.value)}
                      placeholder="Solana address"
                      autoComplete="off"
                      autoCapitalize="none"
                      spellCheck={false}
                      disabled={pending}
                      aria-describedby="recipient-help"
                    />
                    <p id="recipient-help" className="help-text">
                      Use an address with an enabled private wallet.
                    </p>
                  </div>
                ) : (
                  <p className="destination">
                    {action === "Deposit"
                      ? "From your connected wallet to your private balance."
                      : "From your private balance to your connected wallet."}
                  </p>
                )}
              </div>
            </MotionRegion>
            <button
              className="primary-button"
              type="submit"
              disabled={
                pending ||
                refreshing ||
                Boolean(balanceError) ||
                Boolean(amountError) ||
                (action === "Transfer" && !recipient.trim())
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
                <TransferStepper progress={transferProgress} />
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
    </section>
  );
}
