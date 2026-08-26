// Pausing every screen in the house is the most consequential thing this
// console can do from one control, so the tests are about restraint: it must
// not fire on a stray click, it must tell the truth when some devices are
// offline, and it must not shout at a user who simply changed their mind.
import { afterEach, beforeEach, describe, expect, mock, test } from "bun:test";
import { render, screen, waitFor, act, cleanup, fireEvent } from "@testing-library/react";

import type { Device } from "../types";
// Registers the shared module mocks; must be imported before the component.
import { ApiError, apiCalls, apiImpl, toasts, armChangeMode, resetApiMock, resetUiMocks } from "../test/mockApi";

const { PauseEverything } = await import("./PauseEverything");
const { ConfirmProvider } = await import("../lib/confirm");

function device(id: string, status: Device["status"] = "online", locked = false): Device {
  return {
    id,
    tenant_id: "t",
    name: id,
    hostname: id,
    os: "linux",
    agent_version: "0.5.0",
    status,
    locked,
    lock_pending: false,
    tamper_level: 1,
    public_ip: null,
    last_seen: new Date().toISOString(),
    created_at: new Date().toISOString(),
  };
}

beforeEach(() => {
  resetApiMock();
  resetUiMocks();
  // Change mode is on for these — the pause itself is what's under test.
  armChangeMode();
});

afterEach(cleanup);

function setup(devices: Device[], allPaused = false) {
  const onDone = mock(() => {});
  const onSweep = mock((_: boolean) => {});
  render(
    <ConfirmProvider>
      <PauseEverything
        devices={devices}
        allPaused={allPaused}
        onSweep={onSweep}
        onDone={onDone}
      />
    </ConfirmProvider>,
  );
  return { onDone, onSweep, button: screen.getByRole("button", { name: /pause|resume/i }) };
}

describe("pause everything", () => {
  test("a click that is not held does nothing at all", async () => {
    const { button } = setup([device("a"), device("b")]);

    fireEvent.pointerDown(button);
    fireEvent.pointerUp(button); // released immediately

    await new Promise((r) => setTimeout(r, 120));
    // The whole point of the hold: an accidental tap must not freeze the house.
    expect(apiCalls.locked).toHaveLength(0);
  });

  test("holding past the threshold pauses every device", async () => {
    const { button, onSweep, onDone } = setup([device("a"), device("b")]);

    await act(async () => {
      fireEvent.pointerDown(button);
      // The hold is 600ms of requestAnimationFrame.
      await new Promise((r) => setTimeout(r, 900));
    });

    await waitFor(() => expect(apiCalls.locked.sort()).toEqual(["a", "b"]));
    expect(onDone).toHaveBeenCalled();
    // The freeze is shown sweeping across the grid, not merely reported.
    expect(onSweep).toHaveBeenCalledWith(true);
  });

  test("resuming is a plain tap — undoing a pause needs no ceremony", async () => {
    const { button } = setup([device("a", "online", true)], true);

    fireEvent.pointerDown(button);

    await waitFor(() => expect(apiCalls.unlocked).toEqual(["a"]));
  });

  test("an offline device is reported as queued, not as paused", async () => {
    apiImpl.lockDevice = (id) =>
      Promise.resolve({ command_id: "c", queued: true, delivered: id !== "b" });
    const { button } = setup([device("a"), device("b", "offline")]);

    await act(async () => {
      fireEvent.pointerDown(button);
      await new Promise((r) => setTimeout(r, 900));
    });

    await waitFor(() => expect(toasts).toHaveLength(1));
    // Claiming "every screen is paused" when one never got the message is the
    // exact lie this branch exists to prevent.
    expect(toasts[0].msg).toContain("offline");
    expect(toasts[0].msg).not.toContain("Every screen is paused");
  });

  test("a partial failure says how many actually paused", async () => {
    apiImpl.lockDevice = (id) =>
      id === "b" ? Promise.reject(new Error("nope")) : Promise.resolve({ command_id: "c", queued: true, delivered: true });
    const { button } = setup([device("a"), device("b")]);

    await act(async () => {
      fireEvent.pointerDown(button);
      await new Promise((r) => setTimeout(r, 900));
    });

    await waitFor(() => expect(toasts).toHaveLength(1));
    expect(toasts[0].msg).toBe("1 of 2 paused — 1 failed.");
    expect(toasts[0].tone).toBe("warn");
  });

  test("an untrusted session gets the confirm dialog — and cancelling is silent", async () => {
    // Trust lives at login now; a trusted session pauses with no ceremony.
    // Only when the server itself asks for proof (428) does a dialog appear.
    apiImpl.getChangeMode = () => Promise.resolve({ armed_until: null, extended: false });
    apiImpl.lockDevice = () =>
      Promise.reject(new ApiError("step_up_required", "prove it's you", 428));
    const { button } = setup([device("a")]);

    await act(async () => {
      fireEvent.pointerDown(button);
      await new Promise((r) => setTimeout(r, 900));
    });

    // The confirm dialog opens instead of the pause claiming success.
    const cancel = await screen.findByRole("button", { name: /cancel/i });
    fireEvent.click(cancel);

    await new Promise((r) => setTimeout(r, 60));
    // Changing your mind is not a failure and must not be scolded.
    expect(toasts).toHaveLength(0);
  });
});
