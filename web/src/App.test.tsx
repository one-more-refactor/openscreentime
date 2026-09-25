// The very first impression: the installer prints `https://host/#setup=<code>`.
// With no session yet, the app sends `/` on to `/login` — and that redirect
// replaces the whole URL, fragment and all. The code must survive it, and the
// same goes for a recovery link (`#signin=`) opened on any address.
import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";

// Registers the shared API mock; must be imported before the components.
import { apiCalls, apiImpl, resetApiMock } from "./test/mockApi";

const { BrowserRouter } = await import("react-router-dom");
const { App } = await import("./App");

function openApp() {
  render(
    <BrowserRouter>
      <App />
    </BrowserRouter>,
  );
}

beforeEach(() => {
  resetApiMock();
  apiImpl.getAuthConfig = () =>
    Promise.resolve({ oidc: false, oidc_name: "SSO", needs_setup: true, setup_code_required: true });
  try {
    sessionStorage.clear();
  } catch {
    /* fine */
  }
});
afterEach(() => {
  cleanup();
  window.history.replaceState(null, "", "/");
});

describe("the setup link", () => {
  test("keeps its code through the redirect to the sign-in page", async () => {
    window.history.replaceState(null, "", "/#setup=abc123");
    openApp();
    expect(await screen.findByText("Create your household")).toBeTruthy();
    // Redirected, and the code is out of the address bar…
    expect(window.location.pathname).toBe("/login");
    expect(window.location.hash).toBe("");
    // …but not lost: nothing to type but a name.
    expect(screen.queryByLabelText("Setup code")).toBeNull();
    fireEvent.change(screen.getByLabelText("Your name"), { target: { value: "Philip" } });
    fireEvent.click(screen.getByRole("button", { name: "Create passkey" }));
    await waitFor(() => expect(apiCalls.register).toEqual([["Philip", "abc123"]]));
  });

  test("without a code in the link, the page asks for it", async () => {
    window.history.replaceState(null, "", "/");
    openApp();
    expect(await screen.findByLabelText("Setup code")).toBeTruthy();
  });
});

describe("a recovery link", () => {
  test("is redeemed from any address, even one that redirects", async () => {
    // An address the console doesn't have is sent on to `/` at once.
    window.history.replaceState(null, "", "/recover#signin=f00dCAFE");
    openApp();
    await waitFor(() => expect(apiCalls.link).toEqual(["f00dCAFE"]));
    expect(window.location.hash).toBe("");
  });
});
