// Change mode is the console's one security state, so the tests are about the
// promise it makes: ask once, then get out of the way; lock when told, lock
// when the time runs out, and come back honest after a reload.
import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import { render, screen, waitFor, act, cleanup, fireEvent } from "@testing-library/react";

// Registers the shared API mock; must be imported before the provider.
import { apiCalls, apiImpl, ApiError, MOCK_CODE, armChangeMode, resetApiMock } from "../test/mockApi";

const { ChangeModeProvider, useChangeMode, StepUpCancelled } = await import("./changemode");

const outcomes = { ok: 0, errors: [] as unknown[] };

/** A tiny consumer: shows the state, and buttons that exercise the API. */
function Probe({ fn }: { fn: () => Promise<unknown> }) {
  const cm = useChangeMode();
  return (
    <div>
      <span data-testid="armed">{String(cm.armed)}</span>
      <span data-testid="extended">{String(cm.extended)}</span>
      <button
        onClick={() =>
          void cm
            .guard(fn)
            .then(() => {
              outcomes.ok += 1;
            })
            .catch((e) => outcomes.errors.push(e))
        }
      >
        go
      </button>
      <button onClick={() => void cm.lock()}>lock now</button>
      <button onClick={() => void cm.extend()}>extend now</button>
    </div>
  );
}

function setup(fn: () => Promise<unknown> = () => Promise.resolve("done")) {
  render(
    <ChangeModeProvider>
      <Probe fn={fn} />
    </ChangeModeProvider>,
  );
  return {
    go: screen.getByRole("button", { name: "go" }),
    armed: () => screen.getByTestId("armed").textContent,
    extended: () => screen.getByTestId("extended").textContent,
  };
}

async function typeCode(code: string) {
  const input = await screen.findByLabelText(/code from your authenticator/i);
  fireEvent.change(input, { target: { value: code } });
}

beforeEach(() => {
  resetApiMock();
  outcomes.ok = 0;
  outcomes.errors.length = 0;
});
afterEach(cleanup);

describe("change mode", () => {
  test("a reloaded console restores the state the server reports", async () => {
    armChangeMode();
    const { armed } = setup();
    await waitFor(() => expect(armed()).toBe("true"));
    expect(apiCalls.changeMode).toBe(1);
  });

  test("while on, guard runs the change straight through — no dialog", async () => {
    armChangeMode();
    const { go, armed } = setup();
    await waitFor(() => expect(armed()).toBe("true"));

    fireEvent.click(go);
    await waitFor(() => expect(outcomes.ok).toBe(1));
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(apiCalls.verify).toHaveLength(0);
  });

  test("while off, the first change asks once; the next goes through without asking", async () => {
    const { go, armed } = setup();
    await waitFor(() => expect(apiCalls.changeMode).toBe(1));
    expect(armed()).toBe("false");

    fireEvent.click(go);
    // The dialog — not a 428 error — is what a locked control produces.
    expect(await screen.findByRole("dialog", { name: /turn on change mode/i })).toBeTruthy();
    await typeCode(MOCK_CODE);

    await waitFor(() => expect(outcomes.ok).toBe(1));
    expect(apiCalls.verify).toEqual([MOCK_CODE]);
    await waitFor(() => expect(armed()).toBe("true"));
    // Entering plays the veil over an app that is already unlocked.
    expect(screen.getByTestId("changemode-veil").dataset.kind).toBe("enter");

    fireEvent.click(go);
    await waitFor(() => expect(outcomes.ok).toBe(2));
    expect(apiCalls.verify).toHaveLength(1);
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  test("a wrong code shakes and stays; cancelling rejects with StepUpCancelled", async () => {
    const { go } = setup();
    await waitFor(() => expect(apiCalls.changeMode).toBe(1));

    fireEvent.click(go);
    await screen.findByRole("dialog");
    await typeCode("000000");
    expect(await screen.findByRole("alert")).toBeTruthy();
    expect(outcomes.ok).toBe(0);

    fireEvent.click(screen.getByRole("button", { name: /cancel/i }));
    await waitFor(() => expect(outcomes.errors).toHaveLength(1));
    expect(outcomes.errors[0]).toBeInstanceOf(StepUpCancelled);
  });

  test("a 428 mid-grant (the server's clock, not ours) asks again once and retries", async () => {
    armChangeMode();
    let calls = 0;
    const fn = () => {
      calls += 1;
      return calls === 1
        ? Promise.reject(new ApiError("step_up_required", "this change needs a second factor", 428))
        : Promise.resolve("done");
    };
    const { go, armed } = setup(fn);
    await waitFor(() => expect(armed()).toBe("true"));

    fireEvent.click(go);
    await screen.findByRole("dialog");
    // The grant the server rejected is gone from the UI too.
    expect(armed()).toBe("false");
    await typeCode(MOCK_CODE);
    await waitFor(() => expect(outcomes.ok).toBe(1));
    expect(calls).toBe(2);
  });

  test("lock() locks now, tells the server, and plays the lock veil", async () => {
    armChangeMode();
    const { armed } = setup();
    await waitFor(() => expect(armed()).toBe("true"));

    fireEvent.click(screen.getByRole("button", { name: "lock now" }));
    await waitFor(() => expect(armed()).toBe("false"));
    expect(apiCalls.lockChangeMode).toBe(1);
    expect(screen.getByTestId("changemode-veil").dataset.kind).toBe("lock");
  });

  test("extend() is once: the flag flips and the server was asked", async () => {
    armChangeMode();
    const { armed, extended } = setup();
    await waitFor(() => expect(armed()).toBe("true"));
    expect(extended()).toBe("false");

    fireEvent.click(screen.getByRole("button", { name: "extend now" }));
    await waitFor(() => expect(extended()).toBe("true"));
    expect(apiCalls.extendChangeMode).toBe(1);
  });

  test("when the time runs out it locks by itself, with the lock veil", async () => {
    apiImpl.getChangeMode = () =>
      Promise.resolve({ armed_until: new Date(Date.now() + 250).toISOString(), extended: false });
    const { armed } = setup();
    await waitFor(() => expect(armed()).toBe("true"));

    await act(async () => {
      await new Promise((r) => setTimeout(r, 400));
    });
    expect(armed()).toBe("false");
    expect(screen.getByTestId("changemode-veil").dataset.kind).toBe("lock");
  });
});
