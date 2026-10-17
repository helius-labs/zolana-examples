import {
  AccountRole,
  type AccountSignerMeta,
  type Address,
  type Instruction,
  type TransactionSigner,
} from "@solana/kit";
import {
  encodeTransactInstructionData,
  transactCpiAccounts,
  type TransactInstructionData,
} from "@heliuslabs/zolana/interface";
import type { ProgramOwner } from "@heliuslabs/zolana/transaction";
import { PROGRAM_ID, concat, encoder, u64 } from "./terms.js";
import type { CompressedGroth16Proof } from "./wasm.js";

interface DvpInstructionAccounts {
  readonly payer: TransactionSigner;
  readonly signer: TransactionSigner;
  readonly tree: Address;
  readonly authority: ProgramOwner;
  readonly bump: number;
  readonly dvpId: Uint8Array;
  readonly proof: CompressedGroth16Proof;
  readonly transact: TransactInstructionData;
}

export function createDvpInstruction(input: DvpInstructionAccounts): Promise<Instruction> {
  return dvpInstruction(input, 0, concat(input.dvpId, Uint8Array.of(input.bump)));
}

export function reclaimDvpInstruction(
  input: DvpInstructionAccounts & { creator: Address },
): Promise<Instruction> {
  return dvpInstruction(
    input,
    1,
    concat(encoder.encode(input.creator), input.dvpId, Uint8Array.of(input.bump)),
  );
}

export function settleDvpInstruction(
  input: DvpInstructionAccounts & { creator: Address; currentTimestamp: bigint },
): Promise<Instruction> {
  return dvpInstruction(
    input,
    2,
    concat(
      encoder.encode(input.creator),
      input.dvpId,
      Uint8Array.of(input.bump),
      u64(input.currentTimestamp),
    ),
  );
}

export function cancelDvpInstruction(
  input: DvpInstructionAccounts & { creator: Address },
): Promise<Instruction> {
  return dvpInstruction(
    input,
    3,
    concat(encoder.encode(input.creator), input.dvpId, Uint8Array.of(input.bump)),
  );
}

export function rejectDvpInstruction(
  input: DvpInstructionAccounts & { creator: Address },
): Promise<Instruction> {
  return dvpInstruction(
    input,
    4,
    concat(encoder.encode(input.creator), input.dvpId, Uint8Array.of(input.bump)),
  );
}

async function dvpInstruction(
  input: DvpInstructionAccounts,
  tag: number,
  args: Uint8Array,
): Promise<Instruction> {
  if (input.proof.a.length !== 32 || input.proof.b.length !== 64 || input.proof.c.length !== 32) {
    throw new Error("Invalid DvP proof length");
  }
  if (
    !Number.isInteger(input.bump) ||
    input.bump < 0 ||
    input.bump > 255 ||
    input.dvpId.length !== 32
  ) {
    throw new Error("Invalid DvP instruction arguments");
  }
  const sppAccounts = await transactCpiAccounts({
    payer: input.payer,
    inputTree: input.tree,
    outputTree: input.tree,
    data: input.transact,
    signerPdas: [input.authority.pda],
  });
  const payer: AccountSignerMeta = {
    address: input.payer.address,
    role: AccountRole.WRITABLE_SIGNER,
    signer: input.payer,
  };
  const signer: AccountSignerMeta = {
    address: input.signer.address,
    role: AccountRole.READONLY_SIGNER,
    signer: input.signer,
  };
  return {
    programAddress: PROGRAM_ID,
    accounts: [payer, signer, ...sppAccounts],
    data: concat(
      Uint8Array.of(tag),
      args,
      input.proof.a,
      input.proof.b,
      input.proof.c,
      encodeTransactInstructionData(input.transact),
    ),
  };
}
