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
import { PublicKey } from "@solana/web3.js";
import { SOL_MINT, syncWallet } from "@heliuslabs/zolana";
import { usePrivateWallet } from "./hooks/usePrivateWallet";
import {
  BalanceSyncError,
  DEPOSIT_AMOUNT,
  TRANSFER_AMOUNT,
  WITHDRAW_AMOUNT,
  depositSol,
  transferSol,
  withdrawSol,
} from "./hooks/useDeposit";
import { formatSol } from "./lib/formatSol";
import { withTimeout } from "./lib/withTimeout";

const progress = {
  initializing: "Preparing private wallet…",
  signing: "Approve message in your wallet",
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
  const {
    ready,
    authenticated,
    connected,
    sessionKey,
    login,
    logout,
    createWallet,
  } = useEmbeddedWallet();
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
      await createWallet();
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
              Sign in with Privy to view your balance and test private
              transfers.
            </p>
            <button
              className="primary-button"
              disabled={!ready || creating}
              onClick={() =>
                authenticated ? void createEmbeddedWallet() : login()
              }
            >
              {!ready
                ? "Loading wallet…"
                : creating
                  ? "Creating wallet…"
                  : authenticated
                    ? "Create embedded wallet"
                    : "Sign in with Privy"}
            </button>
            {authenticated && (
              <button className="text-button" onClick={() => void logout()}>
                Sign out
              </button>
            )}
            {loginError && (
              <p className="error-message" role="alert">
                {loginError}
              </p>
            )}
          </section>
        )}
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
            href="https://helius-privacy-demo.fly.dev/"
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
  const [recipient, setRecipient] = useState("");
  const [signature, setSignature] = useState<string | null>(null);
  const [txError, setTxError] = useState<string | null>(null);
  const [pending, setPending] = useState(false);
  const [publicBalance, setPublicBalance] = useState<bigint | null>(null);
  const [privateBalance, setPrivateBalance] = useState<bigint | null>(null);
  const [balanceError, setBalanceError] = useState<string | null>(null);
  const [refreshing, setRefreshing] = useState(false);
  const [copyStatus, setCopyStatus] = useState("");
  const session = useRef(0);
  const operation = useRef(false);
  const balanceRequest = useRef(0);
  const initializing = status in progress;

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
    setPending(false);
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
        withTimeout(
          Promise.resolve().then(() => connection.getBalance(new PublicKey(owner), "confirmed")),
        ).then((value) => {
          if (!Number.isSafeInteger(value))
            throw new Error(
              "Public balance is too large to display precisely.",
            );
          const balance = BigInt(value);
          if (active()) setPublicBalance(balance);
          return balance;
        }),
        withTimeout(
          (async () => {
            if (!ctx) return null;
            if (syncPrivate) await syncWallet(ctx);
            return ctx.wallet.balance(SOL_MINT).amount;
          })(),
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
            .join(" "),
        );
      }
      setRefreshing(false);
      return !failed;
    },
    [connection, owner, ctx],
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
    try {
      let result: { signature: string };
      if (action === "Transfer") {
        let destination;
        try {
          destination = address(recipient.trim());
        } catch {
          throw new Error("Enter a valid Solana recipient address.");
        }
        result = await transferSol(ctx, destination);
      } else {
        result = await (action === "Deposit"
          ? depositSol(ctx)
          : withdrawSol(ctx));
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
            "Refresh balances before making another transaction.",
          );
        }
        setTxError(errorText(e));
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
          <span className="wallet-name">Privy embedded wallet</span>
        </div>
        <button className="text-button" onClick={() => void logout()}>
          Sign out
        </button>
      </div>
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

      <div className="balance-section">
        <span className="eyebrow">Private balance</span>
        <p className="balance" aria-label="Private SOL balance">
          {privateBalance === null ? "—" : formatSol(privateBalance)}{" "}
          <span>SOL</span>
        </p>
        <div className="public-balance-row">
          <span>Public balance</span>
          <span className="numeric">
            {publicBalance === null ? "—" : formatSol(publicBalance)} SOL
          </span>
        </div>
        <div className="refresh-row">
          <button
            className="text-button"
            disabled={pending || initializing || refreshing}
            onClick={() => void refresh()}
          >
            {refreshing ? "Refreshing…" : "Refresh balances"}
          </button>
        </div>
        {balanceError && (
          <p className="error-message" role="alert">
            {balanceError}
          </p>
        )}
      </div>

      {!ready ? (
        <div className="enable-section">
          <h2>Activate your private wallet</h2>
          <p className="help-text">
            Sign a message to activate your private wallet. Activation registers
            your wallet address in an onchain registry, which the SDK looks up
            under the hood before every private transfer. Learn more in the{" "}
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
          <p className="status-message" role="status">
            {initializing ? progress[status as keyof typeof progress] : ""}
          </p>
          {error && (
            <p className="error-message" role="alert">
              {error}
            </p>
          )}
        </div>
      ) : (
        <form
          className="transaction-form"
          onSubmit={(event) => {
            event.preventDefault();
            void run();
          }}
        >
          <fieldset className="action-picker" disabled={pending || refreshing}>
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
                  }}
                />
                <span>{option}</span>
              </label>
            ))}
          </fieldset>
          <div className="amount-row">
            <span>Amount</span>
            <span className="numeric">{formatSol(amounts[action])} SOL</span>
          </div>
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
          <button
            className="primary-button"
            type="submit"
            disabled={
              pending ||
              refreshing ||
              Boolean(balanceError) ||
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
            ) : (
              `${action} ${formatSol(amounts[action])} SOL`
            )}
          </button>
          <div className="transaction-result" role="status">
            {pending && (
              <p>Approve in your wallet, then wait for confirmation.</p>
            )}
            {signature && (
              <>
                <p className="success-message">Transaction confirmed</p>
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
        </form>
      )}
    </section>
  );
}
