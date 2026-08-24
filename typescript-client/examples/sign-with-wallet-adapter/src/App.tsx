import { useState } from "react";
import { useWallet } from "@solana/wallet-adapter-react";
import { WalletMultiButton } from "@solana/wallet-adapter-react-ui";
import { address } from "@solana/kit";
import { SOL_MINT } from "@heliuslabs/zolana";
import { usePrivateWallet } from "./hooks/usePrivateWallet";
import {
  depositSol,
  transferSol,
  withdrawSol,
} from "./hooks/useDeposit";

export default function App() {
  const { connected } = useWallet();
  const { ready, error, ctx, owner } = usePrivateWallet();
  const [recipient, setRecipient] = useState("");
  const [signature, setSignature] = useState<string | null>(null);
  const [txError, setTxError] = useState<string | null>(null);
  const privateBalance = ctx?.wallet.balance(SOL_MINT).amount ?? 0n;

  async function run(action: () => Promise<{ signature: string }>) {
    setTxError(null);
    try {
      const result = await action();
      setSignature(result.signature);
    } catch (e: unknown) {
      setTxError(e instanceof Error ? e.message : String(e));
    }
  }

  if (!connected) {
    return (
      <section>
        <h1>Private balances</h1>
        <p>
          Connect a wallet. One message signature derives viewing and
          nullifier keys. The wallet keeps the Ed25519 secret.
        </p>
        <WalletMultiButton />
      </section>
    );
  }

  return (
    <section>
      <WalletMultiButton />
      <p>From wallet {owner}</p>
      <p>Private SOL {privateBalance.toString()}</p>
      {error ? <p>{error}</p> : null}
      <button
        disabled={!ready || !ctx}
        onClick={() => ctx && run(() => depositSol(ctx))}
      >
        Deposit 1 SOL
      </button>
      <input
        value={recipient}
        onChange={(e) => setRecipient(e.target.value)}
        placeholder="Recipient Solana address"
      />
      <button
        disabled={!ready || !ctx || !recipient}
        onClick={() =>
          ctx && run(() => transferSol(ctx, address(recipient)))
        }
      >
        Transfer 0.3 SOL
      </button>
      <button
        disabled={!ready || !ctx}
        onClick={() => ctx && run(() => withdrawSol(ctx))}
      >
        Withdraw 0.3 SOL
      </button>
      {signature ? <p>Last signature {signature}</p> : null}
      {txError ? <p>{txError}</p> : null}
    </section>
  );
}
