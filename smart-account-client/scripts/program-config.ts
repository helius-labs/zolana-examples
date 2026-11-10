/**
 * Writes the Squads `ProgramConfig` account that `createSmartAccount` reads,
 * as a validator account file. The release localnet loads the Squads program
 * but not this account. Usage: tsx scripts/program-config.ts <account-dir>
 */
import { mkdir, writeFile } from "node:fs/promises";
import { join } from "node:path";

import { getAddressEncoder } from "@solana/kit";

import {
  PROGRAM_CONFIG_DISCRIMINATOR,
  SMART_ACCOUNT_PROGRAM_ADDRESS,
  localnetTreasuryAddress,
  programConfigAddress,
} from "../src/squads.js";

const PROGRAM_CONFIG_LEN = 160;
const TREASURY_OFFSET = 64;

const accountDir = process.argv[2];
if (accountDir === undefined) {
  throw new Error("usage: tsx scripts/program-config.ts <account-dir>");
}

// smart_account_index 0, authority and creation fee left at zero. The
// treasury only has to match what the config names.
const data = new Uint8Array(PROGRAM_CONFIG_LEN);
data.set(PROGRAM_CONFIG_DISCRIMINATOR, 0);
data.set(
  getAddressEncoder().encode(await localnetTreasuryAddress()),
  TREASURY_OFFSET,
);

const account = {
  pubkey: await programConfigAddress(),
  account: {
    lamports: 1_000_000_000,
    data: [Buffer.from(data).toString("base64"), "base64"],
    owner: SMART_ACCOUNT_PROGRAM_ADDRESS,
    executable: false,
    rentEpoch: Number.MAX_SAFE_INTEGER,
    space: PROGRAM_CONFIG_LEN,
  },
};

await mkdir(accountDir, { recursive: true });
await writeFile(
  join(accountDir, "squads_program_config.json"),
  JSON.stringify(account),
);
console.log(`wrote Squads ProgramConfig ${account.pubkey} to ${accountDir}`);
