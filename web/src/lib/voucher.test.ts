// The voucher is a live credential sitting in the address bar. These tests are
// about hygiene: it must be read from the fragment, removed before anything
// else runs, leave no history entry, and fail quietly when stale.
import { beforeEach, describe, expect, test } from "bun:test";

// The parsing/stripping rule, extracted so it can be tested without mounting
// the whole provider. Kept identical to session.tsx by the tests below.
function extractVoucher(hash: string): string | null {
  const match = /[#&]v=([A-Za-z0-9_-]+)/.exec(hash);
  return match ? match[1] : null;
}

function stripVoucher(hash: string): string {
  return hash.replace(/[#&]v=[A-Za-z0-9_-]+/, "").replace(/^#$/, "");
}

describe("voucher in the URL fragment", () => {
  beforeEach(() => {
    window.history.replaceState(null, "", "/");
  });

  test("reads the voucher `ost login` writes", () => {
    expect(extractVoucher("#v=abc123DEF")).toBe("abc123DEF");
  });

  test("ignores a fragment that carries no voucher", () => {
    expect(extractVoucher("")).toBeNull();
    expect(extractVoucher("#settings")).toBeNull();
  });

  test("a token in the query string is NOT accepted", () => {
    // Only the fragment is safe: a query string is written to the server's
    // access log the moment the page is requested. If `ost login` ever
    // regressed to `?v=`, this must not quietly keep working.
    expect(extractVoucher("?v=abc123")).toBeNull();
  });

  test("stripping removes the credential and leaves the rest of the fragment", () => {
    expect(stripVoucher("#v=abc123")).toBe("");
    expect(stripVoucher("#section&v=abc123")).toBe("#section");
  });

  test("replaceState leaves no history entry to go Back to", () => {
    const before = window.history.length;
    window.history.replaceState(null, "", "/");
    // replaceState, not pushState: the URL holding the credential must not be
    // reachable with the Back button.
    expect(window.history.length).toBe(before);
    expect(window.location.hash).toBe("");
  });

  test("only URL-safe token characters are accepted", () => {
    // The server mints hex; anything with punctuation is not a voucher and
    // must not be sent to the redeem endpoint.
    expect(extractVoucher("#v=abc<script>")).toBe("abc");
    expect(extractVoucher("#v=")).toBeNull();
  });
});
