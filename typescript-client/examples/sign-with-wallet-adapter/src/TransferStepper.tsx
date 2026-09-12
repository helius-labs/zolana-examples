import {
  transferStepIndex,
  type TransferProgress,
} from "./lib/transferProgress";
import "./TransferStepper.css";

const labels = ["Preparing", "Proving", "Sending", "Confirmed"];

/** Mirrors the demo's milestones, driven only by SDK and submission events. */
export function TransferStepper({ progress }: { progress: TransferProgress }) {
  const { stage, failed, marks } = progress;
  const active = transferStepIndex[stage];
  const confirmed = active === 3;
  const done = stage === "done";
  const detail = failed
    ? confirmed
      ? "Balance refresh failed. Your transfer is confirmed."
      : `${labels[active]} failed. Try again.`
    : stage === "signing"
    ? "Approve the transaction in your wallet."
    : stage === "sending"
    ? "Waiting for Solana confirmation…"
    : stage === "confirmed"
    ? "Updating private balance…"
    : done
    ? null
    : stage === "proving"
    ? "Generating your private transfer proof…"
    : "Looking up the recipient and preparing the transfer…";

  const durations = [0, 0, 0, 0];
  for (let i = 0; i < marks.length - 1; i++) {
    durations[transferStepIndex[marks[i].stage]] += Math.max(
      0,
      marks[i + 1].at - marks[i].at
    );
  }

  return (
    <div className="transfer-progress" aria-label="Transfer progress">
      <ol className="transfer-steps">
        {labels.map((label, index) => {
          const isFailed = failed && index === active && !confirmed;
          const complete =
            index < active || (index === active && (done || confirmed));
          const current = index === active && !done;
          const state = isFailed
            ? "failed"
            : complete
            ? "complete"
            : current
            ? "current"
            : "upcoming";
          return (
            <li
              key={label}
              className={`transfer-step ${state}`}
              aria-current={current ? "step" : undefined}
            >
              {index > 0 && (
                <span
                  className={`transfer-connector ${
                    index <= active ? "complete" : ""
                  }`}
                  aria-hidden="true"
                />
              )}
              <span className="transfer-dot" aria-hidden="true" />
              <span className="transfer-label">
                {isFailed ? "Failed" : label}
              </span>
              <span className="transfer-duration">
                {index < active || done
                  ? `${(durations[index] / 1000).toFixed(1)}s`
                  : "\u00a0"}
              </span>
            </li>
          );
        })}
      </ol>
      {detail && (
        <p className="transfer-detail" role="status">
          {detail}
        </p>
      )}
    </div>
  );
}
