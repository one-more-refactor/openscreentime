// "Confirm it's you" guards only the keys. What matters: it accepts the two
// things you sign in with (a passkey, a code on your computer), and it is never
// a dead end — an account with neither is told to sign in again.
import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";

import { apiImpl, MOCK_CODE, resetApiMock } from "../test/mockApi";

const { ConfirmProvider, useConfirm } = await import("./confirm");

function Probe() {
  const { requireConfirm, armed } = useConfirm();
  return (
    <>
      <button onClick={() => void requireConfirm().catch(() => undefined)}>show the keys</button>
      <p>{armed ? "open" : "shut"}</p>
    </>
  );
}

function setup() {
  render(
    <ConfirmProvider>
      <Probe />
    </ConfirmProvider>,
  );
}

beforeEach(resetApiMock);
afterEach(cleanup);

describe("confirm it's you", () => {
  test("a code from your own computer opens the window", async () => {
    setup();
    fireEvent.click(screen.getByRole("button", { name: "show the keys" }));
    fireEvent.click(await screen.findByRole("button", { name: "Get a code on your computer" }));
    const ring = await screen.findByLabelText("The code from your computer");
    fireEvent.change(ring, { target: { value: MOCK_CODE } });
    await waitFor(() => expect(screen.getByText("open")).toBeTruthy());
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  test("a passkey is offered when the account has one", async () => {
    apiImpl.getConfirmStatus = () =>
      Promise.resolve({ armed_until: null, passkey: true, computer: false });
    setup();
    fireEvent.click(screen.getByRole("button", { name: "show the keys" }));
    fireEvent.click(await screen.findByRole("button", { name: /use your passkey/i }));
    await waitFor(() => expect(screen.getByText("open")).toBeTruthy());
  });

  test("with no passkey and no computer online it is not a dead end", async () => {
    apiImpl.getConfirmStatus = () =>
      Promise.resolve({ armed_until: null, passkey: false, computer: false });
    setup();
    fireEvent.click(screen.getByRole("button", { name: "show the keys" }));
    expect(await screen.findByRole("button", { name: "Sign in again" })).toBeTruthy();
    // No code field that could never succeed.
    expect(screen.queryByLabelText("The code from your computer")).toBeNull();
  });
});
