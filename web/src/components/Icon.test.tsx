// One icon set, one source: brand/icons/*.svg. What matters: every file in the
// set is available by its file name, the console draws the file exactly as it
// is (never a redrawn copy), and an icon is silent unless it stands alone.
import { afterEach, describe, expect, test } from "bun:test";
import { cleanup, render } from "@testing-library/react";
import { readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";
import { ICONS, Icon, type IconName } from "./Icon";

const DIR = join(import.meta.dir, "../../../brand/icons");

afterEach(cleanup);

describe("the icon set", () => {
  test("every brand icon is available, and nothing else", () => {
    const files = readdirSync(DIR)
      .filter((f) => f.endsWith(".svg"))
      .map((f) => f.replace(/\.svg$/, ""))
      .sort();
    expect(Object.keys(ICONS).sort()).toEqual(files);
  });

  test("each one is the file itself: monoline, currentColor", () => {
    for (const name of Object.keys(ICONS) as IconName[]) {
      expect(ICONS[name]).toBe(readFileSync(join(DIR, `${name}.svg`), "utf8"));
      expect(ICONS[name]).toContain('stroke="currentColor"');
      expect(ICONS[name]).toContain('stroke-width="2"');
    }
  });
});

describe("<Icon>", () => {
  test("draws the file at the asked size, hidden from screen readers", () => {
    const { container } = render(<Icon name="check" size={18} />);
    const el = container.querySelector(".ic") as HTMLElement;
    expect(el.getAttribute("aria-hidden")).toBe("true");
    expect(el.style.width).toBe("18px");
    expect(el.querySelector("svg path")?.getAttribute("d")).toBe("M5 12.5l4.5 4.5L19 7");
  });

  test("speaks when it stands alone", () => {
    const { container } = render(<Icon name="sign-out" label="Sign out" />);
    const el = container.querySelector(".ic") as HTMLElement;
    expect(el.getAttribute("role")).toBe("img");
    expect(el.getAttribute("aria-label")).toBe("Sign out");
    expect(el.getAttribute("aria-hidden")).toBeNull();
  });
});
