// The unlock code is the parent's key to a child's computer, read live from
// the console. What matters: it is never shown without change mode, it rolls
// over when the server says it does, and recovery codes are shown once.
import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import { render, screen, waitFor, cleanup, fireEvent, act, within } from "@testing-library/react";

// Registers the shared API mock; must be imported before the components.
import { apiCalls, apiImpl, armChangeMode, resetApiMock } from "../test/mockApi";

const { ChangeModeProvider } = await import("../lib/changemode");
const { UnlockCodePanel } = await import("./UnlockCodePanel");

const laptop = { id: "d1", name: "Mia's laptop", status: "online" as const, last_seen: new Date().toISOString() };

function setup(props: Partial<Parameters<typeof UnlockCodePanel>[0]> = {}) {
  render(
    <ChangeModeProvider>
      <UnlockCodePanel device={laptop} {...props} />
    </ChangeModeProvider>,
  );
}

beforeEach(resetApiMock);
afterEach(cleanup);

describe("unlock code panel", () => {
  test("shows the live code, spaced for reading aloud, with the seconds left", async () => {
    armChangeMode();
    setup({ autoShow: true, variant: "step" });
    await waitFor(() => expect(screen.getByText("123 456")).toBeTruthy());
    expect(apiCalls.unlockCode).toEqual(["d1"]);
    expect(screen.getByText(/changes in 20s/i)).toBeTruthy();
  });

  test("refetches when the code rolls over", async () => {
    armChangeMode();
    apiImpl.getUnlockCode = (id) =>
      Promise.resolve({ code: apiCalls.unlockCode.length > 1 ? "777888" : "123456", seconds_left: 1, period: 30, device_name: id });
    setup({ autoShow: true, variant: "step" });
    await waitFor(() => expect(screen.getByText("123 456")).toBeTruthy());

    await act(async () => {
      await new Promise((r) => setTimeout(r, 1300));
    });
    await waitFor(() => expect(screen.getByText("777 888")).toBeTruthy());
    expect(apiCalls.unlockCode.length).toBeGreaterThanOrEqual(2);
  });

  test("without change mode the code is not shown — the dialog is", async () => {
    setup({ autoShow: true, variant: "step" });
    expect(await screen.findByRole("dialog", { name: /turn on change mode/i })).toBeTruthy();
    expect(apiCalls.unlockCode).toHaveLength(0);
    expect(screen.queryByText("123 456")).toBeNull();

    fireEvent.click(screen.getByRole("button", { name: /cancel/i }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    // Cancelling folds the live panel away instead of leaving "Reading…".
    expect(screen.queryByTestId("unlock-code-live")).toBeNull();
  });

  test("the row hides the code until asked, then shows it", async () => {
    armChangeMode();
    setup();
    await waitFor(() => expect(apiCalls.changeMode).toBe(1));
    expect(screen.queryByTestId("unlock-code-live")).toBeNull();

    fireEvent.click(screen.getByRole("button", { name: /show code/i }));
    await waitFor(() => expect(screen.getByText("123 456")).toBeTruthy());
    expect(screen.getByRole("button", { name: /hide code/i })).toBeTruthy();
  });

  test("recovery codes: eight of them, shown once, in a sheet", async () => {
    armChangeMode();
    setup();
    await waitFor(() => expect(apiCalls.changeMode).toBe(1));

    fireEvent.click(screen.getByRole("button", { name: /recovery codes/i }));
    const sheet = await screen.findByRole("dialog", { name: /recovery codes/i });
    expect(sheet).toBeTruthy();
    expect(apiCalls.generateRecovery).toEqual(["d1"]);
    expect(screen.getAllByText(/1234 567\d/)).toHaveLength(8);
  });

  test("replacing warns, then re-keys and says the recovery codes are gone", async () => {
    armChangeMode();
    // After the re-key the server hands out codes from the new secret.
    apiImpl.getUnlockCode = (id) =>
      Promise.resolve({ code: apiCalls.rotate.length ? "654321" : "123456", seconds_left: 20, period: 30, device_name: id });
    setup();
    await waitFor(() => expect(apiCalls.changeMode).toBe(1));

    fireEvent.click(screen.getByRole("button", { name: /^replace$/i }));
    const confirm = await screen.findByRole("dialog", { name: /replace unlock code/i });
    expect(confirm).toBeTruthy();
    expect(apiCalls.rotate).toHaveLength(0);

    fireEvent.click(within(confirm).getByRole("button", { name: /^replace$/i }));
    await waitFor(() => expect(apiCalls.rotate).toEqual(["d1"]));
    await waitFor(() => expect(screen.getByText("654 321")).toBeTruthy());
    expect(screen.getByRole("status").textContent).toMatch(/recovery codes are gone/i);
  });
});
