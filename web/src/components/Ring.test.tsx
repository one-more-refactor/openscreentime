// The ring is the brand: time used today, from the tick at twelve, clockwise.
// What matters: the fraction lands where a clock hand would, the tick is there
// on anything 40 px and up, a pause is dashed (never red), no limit is an empty
// track, and red appears only when the ring is full.
import { afterEach, describe, expect, test } from "bun:test";
import { cleanup, render } from "@testing-library/react";
import { Ring, RingNumber, arcPath, ringGeometry, ringPoint, ringTone } from "./Ring";

afterEach(cleanup);

function ring(props: Parameters<typeof Ring>[0]) {
  const { container } = render(<Ring {...props} />);
  return container.querySelector(".time-ring") as HTMLElement;
}

describe("geometry", () => {
  test("follows the board: stroke, tick and radius for a 64 px ring", () => {
    const g = ringGeometry(64);
    expect(g.sw).toBe(5); // round(64 × 0.08)
    expect(g.hasTick).toBe(true);
    // Pulled in a little so the tick can overhang the track.
    expect(g.r).toBeCloseTo(64 / 2 - 5 / 2 - 5 * 0.35, 5);
    expect(g.tickLen).toBe(8);
  });

  test("a fraction lands where a clock hand would, sweeping clockwise", () => {
    // A quarter used = three o'clock.
    const [x, y] = ringPoint(50, 40, 0.25);
    expect(x).toBeCloseTo(90, 5);
    expect(y).toBeCloseTo(50, 5);
    // Half = six o'clock.
    const [hx, hy] = ringPoint(50, 40, 0.5);
    expect(hx).toBeCloseTo(50, 5);
    expect(hy).toBeCloseTo(90, 5);
    // The arc starts at the tick (twelve), sweep flag 1 = clockwise, and
    // takes the long way round only past half.
    expect(arcPath(50, 40, 0.25)).toBe("M50.00 10.00A40.00 40.00 0 0 1 90.00 50.00");
    expect(arcPath(50, 40, 0.75)).toBe("M50.00 10.00A40.00 40.00 0 1 1 10.00 50.00");
  });

  test("green, amber at 15 minutes or less, red only when full", () => {
    expect(ringTone(0.4, 50)).toBe("brand");
    expect(ringTone(0.9, 15)).toBe("warn");
    expect(ringTone(0.99, 1)).toBe("warn");
    expect(ringTone(1, 0)).toBe("stop");
  });
});

describe("the ring", () => {
  test("draws time used as an arc from the tick", () => {
    const el = ring({ size: 64, used: 0.25 });
    const g = ringGeometry(64);
    const arc = el.querySelector(".ring-arc");
    expect(arc).toBeTruthy();
    expect(arc?.getAttribute("d")).toBe(arcPath(g.c, g.r, 0.25));
    expect(arc?.getAttribute("stroke")).toBe("var(--brand)");
    // A round end on the arc, none at the tick.
    expect(el.querySelector(".ring-cap")).toBeTruthy();
    expect(el.dataset.state).toBe("fill");
  });

  test("has its tick at 40 px and up, and none below", () => {
    expect(ring({ size: 40, used: 0.3 }).querySelector(".ring-tick")).toBeTruthy();
    cleanup();
    expect(ring({ size: 140, used: 0.3 }).querySelector(".ring-tick")).toBeTruthy();
    cleanup();
    expect(ring({ size: 28, used: 0.3 }).querySelector(".ring-tick")).toBeNull();
  });

  test("the tick sits at twelve o'clock", () => {
    const g = ringGeometry(64);
    const d = ring({ size: 64, used: 0.3 }).querySelector(".ring-tick")?.getAttribute("d") ?? "";
    // "M{cx} {top}v{len}": centred horizontally, straddling the top of the ring.
    const [, x, y, len] = d.match(/^M([\d.]+) ([\d.]+)v([\d.]+)$/) ?? [];
    expect(Number(x)).toBe(g.c);
    expect(Number(y) + Number(len) / 2).toBeCloseTo(g.c - g.r, 1);
  });

  test("a pause is a dashed ring in ink-3 — no arc, never red", () => {
    const el = ring({ size: 64, used: 0.6, paused: true });
    const dash = el.querySelector(".ring-dash");
    expect(dash).toBeTruthy();
    expect(dash?.getAttribute("stroke-dasharray")).toMatch(/^[\d.]+ [\d.]+$/);
    expect(dash?.getAttribute("stroke")).toBe("var(--ink-3)");
    expect(el.querySelector(".ring-arc")).toBeNull();
    expect(el.innerHTML).not.toContain("var(--stop)");
    expect(el.dataset.state).toBe("paused");
  });

  test("no limit is the empty track, with the tick", () => {
    const el = ring({ size: 64, used: null });
    expect(el.querySelector(".ring-track")).toBeTruthy();
    expect(el.querySelector(".ring-arc")).toBeNull();
    expect(el.querySelector(".ring-tick")).toBeTruthy();
    expect(el.dataset.state).toBe("none");
  });

  test("time's up is the full ring, in red", () => {
    const el = ring({ size: 64, used: 1 });
    expect(el.querySelector(".ring-full")?.getAttribute("stroke")).toBe("var(--stop)");
    cleanup();
    // Zero minutes left is full, whatever the rounding of `used` says.
    const z = ring({ size: 64, used: 0.97, minutesLeft: 0 });
    expect(z.querySelector(".ring-full")).toBeTruthy();
  });

  test("amber at 15 minutes or less", () => {
    const el = ring({ size: 64, used: 0.8, minutesLeft: 12 });
    expect(el.querySelector(".ring-arc")?.getAttribute("stroke")).toBe("var(--warn)");
  });

  test("the track follows what it sits on", () => {
    expect(ring({ size: 64, used: 0.2 }).querySelector(".ring-track")?.getAttribute("stroke")).toBe("var(--line)");
    cleanup();
    expect(ring({ size: 64, used: 0.2, on: "paper" }).querySelector(".ring-track")?.getAttribute("stroke")).toBe(
      "var(--line-2)",
    );
  });

  test("is decoration unless it is given words", () => {
    expect(ring({ size: 64, used: 0.2 }).getAttribute("aria-hidden")).toBe("true");
    cleanup();
    const spoken = ring({ size: 64, used: 0.2, label: "27 minutes left" });
    expect(spoken.getAttribute("role")).toBe("img");
    expect(spoken.getAttribute("aria-label")).toBe("27 minutes left");
  });
});

describe("a number inside the ring", () => {
  test("three characters at 0.29 × the diameter, longer ones at 0.2 ×", () => {
    const { container, rerender } = render(<RingNumber size={140} value="27" unit="min left" />);
    expect((container.querySelector(".ring-num") as HTMLElement).style.fontSize).toBe("41px");
    rerender(<RingNumber size={140} value="1 h 12" unit="min left" />);
    expect((container.querySelector(".ring-num") as HTMLElement).style.fontSize).toBe("28px");
  });
});
