import express, { type ErrorRequestHandler } from "express";
import { PORT } from "./lib/config.js";
import { deposit } from "./handlers/transactions/deposit.js";
import { transfer } from "./handlers/transactions/transfer.js";
import { withdraw } from "./handlers/transactions/withdraw.js";
import { createWallet } from "./handlers/wallets/create_wallet.js";
import { getWallet } from "./handlers/wallets/get_wallet.js";

const app = express();
app.use(express.json());

app.post("/wallets", createWallet);
app.get("/wallets/:address", getWallet);

app.post("/wallets/:address/deposit", deposit);
app.post("/wallets/:address/transfer", transfer);
app.post("/wallets/:address/withdraw", withdraw);

const onError: ErrorRequestHandler = (error, _req, res, _next) => {
  console.error(error);
  const message = error instanceof Error ? error.message : String(error);
  res.status(500).json({ error: message });
};
app.use(onError);

app.listen(PORT, () => {
  console.log(`Private wallet starter listening on http://localhost:${PORT}`);
});
