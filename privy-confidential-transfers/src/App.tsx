import { useState } from "react";
import { usePrivy } from "@privy-io/react-auth";
import {
  useWallets,
  useCreateWallet,
  useSignMessage,
  useSignTransaction,
  type ConnectedStandardSolanaWallet,
} from "@privy-io/react-auth/solana";
import { clientConfig } from "./config.js";
import { connect, validateTransfers } from "./flow.js";
import type { RemoteWallet } from "./signer.js";

export function App() {
  const { ready, authenticated, login, logout } = usePrivy();
  const { wallets } = useWallets();
  const { createWallet } = useCreateWallet();
  const { signMessage } = useSignMessage();
  const { signTransaction } = useSignTransaction();
  const [busy, setBusy] = useState(false);
  const [lines, setLines] = useState<string[]>([]);
  const [result, setResult] = useState<Awaited<ReturnType<typeof validateTransfers>>>();

  function remote(wallet: ConnectedStandardSolanaWallet): RemoteWallet {
    return {
      address: wallet.address,
      async signMessage(message) {
        return (await signMessage({ wallet, message })).signature;
      },
      async signTransaction(transaction) {
        return (await signTransaction({ wallet, transaction, chain: "solana:devnet" }))
          .signedTransaction;
      },
    };
  }

  async function run() {
    setBusy(true);
    setLines([]);
    setResult(undefined);
    try {
      const config = clientConfig(`${location.origin}/rpc`);
      const client = await connect(config);
      const report = await validateTransfers(
        client,
        remote(wallets[0]!),
        remote(wallets[1]!),
        (stage, details) =>
          setLines((lines) => [
            ...lines,
            `${stage}${details ? ` ${JSON.stringify(details)}` : ""}`,
          ]),
      );
      setResult(report);
    } catch (error) {
      setLines((lines) => [...lines, error instanceof Error ? error.message : "Validation failed"]);
    } finally {
      setBusy(false);
    }
  }

  async function addWallet() {
    try {
      await createWallet({ createAdditional: true });
    } catch (error) {
      setLines([error instanceof Error ? error.message : "Wallet creation failed"]);
    }
  }

  return (
    <main>
      <p>Solana Devnet</p>
      <h1>Privy confidential transfers</h1>
      <p>
        Deposit 0.01 SOL, transfer 0.003 SOL confidentially to a second Privy wallet, then withdraw
        both private balances.
      </p>
      {!authenticated ? (
        <button disabled={!ready} onClick={login}>
          Log in with Privy
        </button>
      ) : (
        <>
          <button disabled={busy} onClick={() => logout()}>
            Log out
          </button>
          {wallets.length < 2 && <button onClick={addWallet}>Create recipient wallet</button>}
          {wallets.slice(0, 2).map((wallet, i) => (
            <p key={wallet.address}>
              {i ? "Recipient" : "Sender"}: <code>{wallet.address}</code>
            </p>
          ))}
          <p>Fund each address with at least 0.05 Devnet SOL before running.</p>
          <button disabled={busy || wallets.length < 2} onClick={run}>
            {busy ? "Validating…" : "Validate on Devnet"}
          </button>
        </>
      )}
      <ol aria-live="polite">
        {lines.map((line, i) => (
          <li key={i}>{line}</li>
        ))}
      </ol>
      {result && (
        <>
          <h2>Validation passed</h2>
          <p>
            Both private balances are zero. The recipient decrypted and withdrew the confidential
            transfer.
          </p>
          {Object.entries(result.transactions).map(([stage, tx]) => (
            <p key={stage}>
              <a
                href={`https://explorer.solana.com/tx/${tx.signature}?cluster=devnet`}
                target="_blank"
                rel="noreferrer"
              >
                {stage}
              </a>
            </p>
          ))}
        </>
      )}
    </main>
  );
}
