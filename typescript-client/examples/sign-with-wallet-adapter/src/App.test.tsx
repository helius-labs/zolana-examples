// @vitest-environment jsdom
import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Signature } from "@solana/kit";
import { useEmbeddedWallet } from "./hooks/useEmbeddedWallet";
import { useRpcConnection } from "./hooks/useRpcConnection";
import { syncWallet } from "@heliuslabs/zolana";
import { usePrivateWallet } from "./hooks/usePrivateWallet";
import {
  BalanceSyncError,
  depositSol,
  transferSol,
  withdrawSol,
} from "./hooks/useDeposit";
import App from "./App";

vi.mock("./hooks/useEmbeddedWallet", () => ({ useEmbeddedWallet: vi.fn() }));
vi.mock("./hooks/useRpcConnection", () => ({ useRpcConnection: vi.fn() }));
vi.mock("@heliuslabs/zolana", () => ({ SOL_MINT: "sol", syncWallet: vi.fn() }));
vi.mock("./hooks/usePrivateWallet", () => ({ usePrivateWallet: vi.fn() }));
vi.mock("./hooks/useDeposit", async (importOriginal) => {
  const original = await importOriginal<typeof import("./hooks/useDeposit")>();
  return {
    ...original,
    depositSol: vi.fn(),
    transferSol: vi.fn(),
    withdrawSol: vi.fn(),
  };
});
let adapter: ReturnType<typeof useEmbeddedWallet>;
let state: ReturnType<typeof usePrivateWallet>;
const connection = { getBalance: vi.fn() };
let privateLamports = 10_000_000n;

beforeEach(() => {
  vi.clearAllMocks();
  privateLamports = 10_000_000n;
  adapter = {
    connected: true,
    owner: "11111111111111111111111111111111",
    ready: true,
    authenticated: true,
    sessionKey: "user-1:wallet-1",
    login: vi.fn(),
    logout: vi.fn(),
    createWallet: vi.fn(),
  } as unknown as ReturnType<typeof useEmbeddedWallet>;
  state = {
    ready: true,
    status: "ready",
    error: null,
    owner: adapter.owner,
    initialize: vi.fn(),
    ctx: { wallet: { balance: () => ({ amount: privateLamports }) } } as never,
  };
  vi.mocked(useEmbeddedWallet).mockImplementation(() => adapter);
  vi.mocked(useRpcConnection).mockReturnValue(
    connection as unknown as ReturnType<typeof useRpcConnection>,
  );
  vi.mocked(usePrivateWallet).mockImplementation(() => state);
  connection.getBalance.mockResolvedValue(1_000_000_000);
  vi.mocked(syncWallet).mockResolvedValue(undefined as never);
  vi.mocked(depositSol).mockResolvedValue({
    signature: "deposit-signature" as Signature,
    privateBalance: 20_000_000n,
  });
  vi.mocked(transferSol).mockResolvedValue({
    signature: "transfer-signature" as Signature,
    privateBalance: 7_000_000n,
  });
  vi.mocked(withdrawSol).mockResolvedValue({
    signature: "withdraw-signature" as Signature,
    privateBalance: 7_000_000n,
  });
});
afterEach(cleanup);
async function renderReady() {
  const view = render(<App />);
  await waitFor(() =>
    expect(
      (
        screen.getByRole("button", {
          name: "Deposit 0.01 SOL",
        }) as HTMLButtonElement
      ).disabled,
    ).toBe(false),
  );
  return view;
}

describe("wallet interface", () => {
  it("shows a single connect action while disconnected", () => {
    adapter.connected = false;
    adapter.authenticated = false;
    render(<App />);
    expect(
      screen.getByRole("button", { name: "Sign in with Privy" }),
    ).toBeTruthy();
    expect(
      screen.queryByRole("button", { name: "Deposit 0.01 SOL" }),
    ).toBeNull();
    expect(state.initialize).not.toHaveBeenCalled();
  });

  it("requires an explicit click to enable the private wallet", async () => {
    state = { ...state, ready: false, status: "connected", ctx: null };
    render(<App />);
    expect(state.initialize).not.toHaveBeenCalled();
    fireEvent.click(
      screen.getByRole("button", { name: "Activate private wallet" }),
    );
    expect(state.initialize).toHaveBeenCalledTimes(1);
    await waitFor(() => expect(screen.queryByText("Refreshing…")).toBeNull());
    expect(screen.getByLabelText("Private SOL balance").textContent).toContain(
      "—",
    );
  });

  it("formats balances and refreshes without requesting a signature", async () => {
    await renderReady();
    expect(screen.getByLabelText("Private SOL balance").textContent).toBe(
      "0.01 SOL",
    );
    expect(screen.getByText("1 SOL")).toBeTruthy();
    privateLamports = 123_456_789n;
    fireEvent.click(screen.getByRole("button", { name: "Refresh balances" }));
    await waitFor(() =>
      expect(screen.getByLabelText("Private SOL balance").textContent).toBe(
        "0.123456789 SOL",
      ),
    );
    expect(syncWallet).toHaveBeenCalledTimes(1);
    expect(state.initialize).not.toHaveBeenCalled();
  });

  it("validates a recipient before building a transfer", async () => {
    await renderReady();
    fireEvent.click(screen.getByRole("radio", { name: "Transfer" }));
    fireEvent.change(screen.getByLabelText("Recipient"), {
      target: { value: "not an address" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Transfer 0.003 SOL" }));
    await waitFor(() =>
      expect(screen.getByRole("alert").textContent).toContain("valid Solana"),
    );
    expect(transferSol).not.toHaveBeenCalled();
  });

  it("passes a trimmed recipient and shows a devnet explorer link", async () => {
    await renderReady();
    fireEvent.click(screen.getByRole("radio", { name: "Transfer" }));
    fireEvent.change(screen.getByLabelText("Recipient"), {
      target: { value: ` ${state.owner} ` },
    });
    fireEvent.click(screen.getByRole("button", { name: "Transfer 0.003 SOL" }));
    await waitFor(() =>
      expect(screen.getByText("Transaction confirmed")).toBeTruthy(),
    );
    expect(transferSol).toHaveBeenCalledWith(state.ctx, state.owner);
    expect(
      screen
        .getByRole("link", { name: /View transaction/ })
        .getAttribute("href"),
    ).toBe("https://explorer.solana.com/tx/transfer-signature?cluster=devnet");
  });

  it("prevents duplicate submits while the wallet is signing", async () => {
    let complete!: (value: Awaited<ReturnType<typeof depositSol>>) => void;
    vi.mocked(depositSol).mockReturnValue(
      new Promise((resolve) => {
        complete = resolve;
      }),
    );
    await renderReady();
    const button = screen.getByRole("button", { name: "Deposit 0.01 SOL" });
    fireEvent.click(button);
    fireEvent.click(button);
    expect(depositSol).toHaveBeenCalledTimes(1);
    expect(
      (screen.getByRole("button", { name: "Depositing…" }) as HTMLButtonElement)
        .disabled,
    ).toBe(true);
    await act(async () => {
      complete({
        signature: "one-submit" as Signature,
        privateBalance: 20_000_000n,
      });
    });
  });

  it("discards transaction results after an account change", async () => {
    let complete!: (value: Awaited<ReturnType<typeof depositSol>>) => void;
    vi.mocked(depositSol).mockReturnValue(
      new Promise((resolve) => {
        complete = resolve;
      }),
    );
    const view = await renderReady();
    fireEvent.click(screen.getByRole("button", { name: "Deposit 0.01 SOL" }));
    adapter = {
      ...adapter,
      owner: "So11111111111111111111111111111111111111112",
      sessionKey: "user-1:wallet-2",
    };
    state = {
      ...state,
      owner: adapter.owner,
      ready: false,
      status: "connected",
      ctx: null,
    };
    view.rerender(<App />);
    await act(async () => {
      complete({
        signature: "old-account" as Signature,
        privateBalance: 20_000_000n,
      });
    });
    expect(screen.queryByText("Transaction confirmed")).toBeNull();
    expect(screen.queryByRole("link", { name: /View transaction/ })).toBeNull();
    expect(screen.getByLabelText("Private SOL balance").textContent).toContain(
      "—",
    );
  });

  it("shows a balance error without displaying a false zero and can retry", async () => {
    connection.getBalance.mockRejectedValueOnce(new Error("offline"));
    render(<App />);
    await waitFor(() =>
      expect(screen.getByRole("alert").textContent).toContain(
        "Couldn’t refresh",
      ),
    );
    expect(screen.queryByText("0 SOL")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Refresh balances" }));
    await waitFor(() => expect(screen.queryByRole("alert")).toBeNull());
    expect(screen.getByText("1 SOL")).toBeTruthy();
  });

  it("preserves a confirmed receipt on sync failure and requires a refresh", async () => {
    vi.mocked(depositSol).mockRejectedValueOnce(
      new BalanceSyncError("confirmed-before-sync"),
    );
    await renderReady();
    fireEvent.click(screen.getByRole("button", { name: "Deposit 0.01 SOL" }));
    await waitFor(() =>
      expect(screen.getByText("Transaction confirmed")).toBeTruthy(),
    );
    expect(
      screen
        .getByRole("link", { name: /View transaction/ })
        .getAttribute("href"),
    ).toContain("confirmed-before-sync");
    expect(
      (
        screen.getByRole("button", {
          name: "Deposit 0.01 SOL",
        }) as HTMLButtonElement
      ).disabled,
    ).toBe(true);
    fireEvent.click(screen.getByRole("button", { name: "Refresh balances" }));
    await waitFor(() =>
      expect(
        (
          screen.getByRole("button", {
            name: "Deposit 0.01 SOL",
          }) as HTMLButtonElement
        ).disabled,
      ).toBe(false),
    );
    expect(screen.queryByRole("alert")).toBeNull();
  });
  it("updates public SOL without waiting for a slow private sync", async () => {
    let finishSync!: () => void;
    vi.mocked(syncWallet).mockReturnValue(
      new Promise((resolve) => {
        finishSync = () => resolve(undefined as never);
      }),
    );
    await renderReady();
    connection.getBalance.mockResolvedValue(2_000_000_000);
    fireEvent.click(screen.getByRole("button", { name: "Refresh balances" }));
    await waitFor(() => expect(screen.getByText("2 SOL")).toBeTruthy());
    expect(screen.getByRole("button", { name: "Refreshing…" })).toBeTruthy();
    await act(async () => {
      finishSync();
    });
    expect(
      screen.getByRole("button", { name: "Refresh balances" }),
    ).toBeTruthy();
  });
});
