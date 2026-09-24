// "What can a parent see?" must say what the server does. The server's rule
// (server/src/usage.rs `hub_exposure`, tested per bracket there) arrives as
// `parent_sees`; these are its answers for each bracket.
import { describe, expect, test } from "bun:test";
import { parentSeesSentence } from "./parentSees";

describe("what a parent sees, per bracket", () => {
  test("little, kid and younger teen: apps and sites", () => {
    const s = parentSeesSentence({ apps: true, sites: true });
    expect(s).toBe(
      "Your parents can see how long your computer was used, which apps were open, which sites it looked up, and when the rules kicked in.",
    );
  });

  test("older teen: apps, and it says the sites aren't shown", () => {
    const s = parentSeesSentence({ apps: true, sites: false });
    expect(s).toMatch(/which apps were open/);
    expect(s).not.toMatch(/which sites/);
    expect(s).toMatch(/not the sites you looked up/);
  });

  test("adult or self-managed: minutes only", () => {
    const s = parentSeesSentence({ apps: false, sites: false });
    expect(s).toBe("A parent sees how many minutes you used — not your apps, your sites or your own rules.");
    expect(s).not.toMatch(/rules kicked in/);
  });

  test("no answer from the server: says the most, never less", () => {
    expect(parentSeesSentence(undefined)).toBe(parentSeesSentence({ apps: true, sites: true }));
  });
});
