// The veil is a beat, not a wall: it must hand control back on its own clock,
// and under prefers-reduced-motion that clock is a short fade, not the
// choreography.
import { afterEach, describe, expect, mock, test } from "bun:test";
import { render, screen, cleanup } from "@testing-library/react";
import { ChangeModeVeil, VEIL_MS, VEIL_REDUCED_MS } from "./ChangeModeVeil";

afterEach(() => {
  cleanup();
  // setup.ts installs a matchMedia that never matches; put it back.
  globalThis.matchMedia = ((query: string) => ({
    matches: false,
    media: query,
    onchange: null,
    addListener: () => {},
    removeListener: () => {},
    addEventListener: () => {},
    removeEventListener: () => {},
    dispatchEvent: () => false,
  })) as unknown as typeof matchMedia;
});

function wait(ms: number) {
  return new Promise((r) => setTimeout(r, ms));
}

describe("change-mode veil", () => {
  test("plays its full length, never blocks input, and then hands back", async () => {
    const onDone = mock(() => {});
    render(<ChangeModeVeil kind="enter" onDone={onDone} />);
    const veil = screen.getByTestId("changemode-veil");
    expect(veil.getAttribute("aria-hidden")).toBe("true");
    expect(veil.dataset.reduced).toBe("false");
    expect(veil.style.getPropertyValue("--cm-ms")).toBe(`${VEIL_MS.enter}ms`);

    await wait(VEIL_MS.enter - 200);
    expect(onDone).not.toHaveBeenCalled();
    await wait(300);
    expect(onDone).toHaveBeenCalledTimes(1);
  });

  test("locking is the shorter beat", () => {
    render(<ChangeModeVeil kind="lock" onDone={() => {}} />);
    const veil = screen.getByTestId("changemode-veil");
    expect(veil.dataset.kind).toBe("lock");
    expect(veil.style.getPropertyValue("--cm-ms")).toBe(`${VEIL_MS.lock}ms`);
    expect(VEIL_MS.lock).toBeLessThan(VEIL_MS.enter);
  });

  test("under prefers-reduced-motion it is a 150 ms fade with no choreography", async () => {
    globalThis.matchMedia = ((query: string) => ({
      matches: query.includes("prefers-reduced-motion"),
      media: query,
      onchange: null,
      addListener: () => {},
      removeListener: () => {},
      addEventListener: () => {},
      removeEventListener: () => {},
      dispatchEvent: () => false,
    })) as unknown as typeof matchMedia;

    const onDone = mock(() => {});
    render(<ChangeModeVeil kind="enter" onDone={onDone} />);
    const veil = screen.getByTestId("changemode-veil");
    expect(veil.dataset.reduced).toBe("true");
    expect(veil.style.getPropertyValue("--cm-ms")).toBe(`${VEIL_REDUCED_MS}ms`);

    await wait(VEIL_REDUCED_MS + 100);
    expect(onDone).toHaveBeenCalledTimes(1);
  });
});
