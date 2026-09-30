// One-time tokens (a voucher, a recovery link, the first-run setup code) sit in
// the address bar. These tests are about hygiene: read from the fragment only,
// removed before anything else runs, no history entry left behind.
import { beforeEach, describe, expect, test } from "bun:test";
import { readFragmentParam, stripFragmentParam, takeFromFragment } from "./fragment";

describe("tokens in the URL fragment", () => {
  beforeEach(() => {
    window.history.replaceState(null, "", "/");
  });

  test("reads the voucher `ost login` writes, the recovery link and the setup link", () => {
    expect(readFragmentParam("#v=abc123DEF", "v")).toBe("abc123DEF");
    expect(readFragmentParam("#signin=f00d", "signin")).toBe("f00d");
    expect(readFragmentParam("#setup=0a1b2c", "setup")).toBe("0a1b2c");
  });

  test("ignores a fragment that carries no such token", () => {
    expect(readFragmentParam("", "v")).toBeNull();
    expect(readFragmentParam("#settings", "v")).toBeNull();
    // `v` is not a prefix match for another name.
    expect(readFragmentParam("#vx=1", "v")).toBeNull();
  });

  test("a token in the query string is NOT accepted", () => {
    // Only the fragment is safe: a query string is written to the server's
    // access log the moment the page is requested.
    expect(readFragmentParam("?v=abc123", "v")).toBeNull();
  });

  test("only URL-safe token characters are accepted", () => {
    expect(readFragmentParam("#v=abc<script>", "v")).toBe("abc");
    expect(readFragmentParam("#v=", "v")).toBeNull();
  });

  test("stripping removes the credential and leaves the rest of the fragment", () => {
    expect(stripFragmentParam("#v=abc123", "v")).toBe("");
    expect(stripFragmentParam("#section&v=abc123", "v")).toBe("#section");
    expect(stripFragmentParam("#setup=abc&x=1", "setup")).toBe("#x=1");
  });

  test("taking it clears the address bar without a history entry", () => {
    window.history.replaceState(null, "", "/login#setup=abc123");
    const before = window.history.length;
    expect(takeFromFragment("setup")).toBe("abc123");
    expect(window.location.hash).toBe("");
    expect(window.location.pathname).toBe("/login");
    expect(window.history.length).toBe(before);
    // Once.
    expect(takeFromFragment("setup")).toBeNull();
  });
});
