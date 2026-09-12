export type TransferStage =
  | "preparing"
  | "proving"
  | "signing"
  | "sending"
  | "confirmed"
  | "done";

export type TransferProgressCallback = (
  stage: TransferStage,
  signature?: string
) => void;

export type TransferProgress = {
  stage: TransferStage;
  failed: boolean;
  marks: { stage: TransferStage; at: number }[];
};

export const transferStepIndex: Record<TransferStage, number> = {
  preparing: 0,
  proving: 1,
  signing: 2,
  sending: 2,
  confirmed: 3,
  done: 3,
};
