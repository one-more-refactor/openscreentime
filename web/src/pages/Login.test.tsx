// The sign-in page is the product's front door. What matters: two doors and
// nothing else (a name → a code on your own computer, or a passkey), SSO only
// when the server has it, sentence case, and a first run that just works from
// the installer's link.
import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";

// Registers the shared API mock; must be imported before the components.
import { apiCalls, apiImpl, MOCK_CODE, resetApiMock } from "../test/mockApi";

const { MemoryRouter, Route, Routes } = await import("react-router-dom");
const { SessionProvider } = await import("../lib/session");
const { Login } = await import("./Login");

function setup() {
  render(
    <MemoryRouter initialEntries={["/login"]}>
      <SessionProvider>
        <Routes>
          <Route path="/login" element={<Login />} />
          <Route path="/" element={<p>the family</p>} />
        </Routes>
      </SessionProvider>
    </MemoryRouter>,
  );
}

function firstRun(setupCodeRequired: boolean) {
  apiImpl.getAuthConfig = () =>
    Promise.resolve({
      oidc: false,
      oidc_name: "SSO",
      needs_setup: true,
      setup_code_required: setupCodeRequired,
    });
}

beforeEach(() => {
  resetApiMock();
  window.history.replaceState(null, "", "/");
  try {
    sessionStorage.clear();
  } catch {
    /* fine */
  }
});
afterEach(cleanup);

describe("sign in", () => {
  test("two doors, nothing else — and no shouting", async () => {
    setup();
    expect(await screen.findByLabelText("Your name")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Continue" })).toBeTruthy();
    expect(screen.getByRole("button", { name: /sign in with a passkey/i })).toBeTruthy();
    // Exactly one text field; no SSO button unless the server has SSO.
    expect(screen.getAllByRole("textbox")).toHaveLength(1);
    expect(screen.queryByRole("button", { name: /sso/i })).toBeNull();
    expect(screen.getAllByRole("button")).toHaveLength(2);
    // Sentence case: no ALL-CAPS words anywhere on the page.
    expect(document.body.textContent ?? "").not.toMatch(/\b[A-Z]{4,}\b/);
  });

  test("SSO appears only when the server has it", async () => {
    apiImpl.getAuthConfig = () =>
      Promise.resolve({ oidc: true, oidc_name: "Authentik", needs_setup: false, setup_code_required: false });
    setup();
    expect(await screen.findByRole("button", { name: "Sign in with Authentik" })).toBeTruthy();
  });

  test("your name, then the code from your computer, signs you in", async () => {
    setup();
    fireEvent.change(await screen.findByLabelText("Your name"), { target: { value: "Mia" } });
    fireEvent.click(screen.getByRole("button", { name: "Continue" }));

    const ring = await screen.findByLabelText("The code from your computer");
    expect(apiCalls.codeStart).toEqual(["Mia"]);
    expect(screen.getByText(/enter the code from your computer/i)).toBeTruthy();

    // A wrong code says so, in a sentence, and lets you type again.
    fireEvent.change(ring, { target: { value: "000000" } });
    expect(await screen.findByRole("alert")).toBeTruthy();
    expect(screen.getByRole("alert").textContent).toMatch(/^That code didn't match/);

    fireEvent.change(screen.getByLabelText("The code from your computer"), {
      target: { value: MOCK_CODE },
    });
    expect(await screen.findByText("the family")).toBeTruthy();
    expect(apiCalls.codeVerify).toEqual(["000000", MOCK_CODE]);
  });

  test("a passkey is one tap — no name first", async () => {
    setup();
    fireEvent.click(await screen.findByRole("button", { name: /sign in with a passkey/i }));
    expect(await screen.findByText("the family")).toBeTruthy();
    expect(apiCalls.passkey).toBe(1);
  });
});

describe("first run", () => {
  test("the installer's link carries the setup code: just your name, then a passkey", async () => {
    window.history.replaceState(null, "", "/#setup=abc123");
    firstRun(true);
    setup();
    expect(await screen.findByText("Create your household")).toBeTruthy();
    // The code came from the link, so there's nothing to type but a name…
    expect(screen.queryByLabelText("Setup code")).toBeNull();
    // …and it left the address bar.
    expect(window.location.hash).toBe("");

    fireEvent.change(screen.getByLabelText("Your name"), { target: { value: "Philip" } });
    fireEvent.click(screen.getByRole("button", { name: "Create passkey" }));
    expect(await screen.findByText("the family")).toBeTruthy();
    expect(apiCalls.register).toEqual([["Philip", "abc123"]]);
  });

  test("without the link it asks for the setup code", async () => {
    firstRun(true);
    setup();
    expect(await screen.findByLabelText("Setup code")).toBeTruthy();
  });

  test("a local checkout with no setup code asks only for a name", async () => {
    firstRun(false);
    setup();
    expect(await screen.findByText("Create your household")).toBeTruthy();
    expect(screen.queryByLabelText("Setup code")).toBeNull();
  });
});

describe("the page still works when the server can't be asked", () => {
  test("it falls back to the two doors", async () => {
    apiImpl.getAuthConfig = () => Promise.reject(new Error("offline"));
    setup();
    await waitFor(() => expect(screen.getByLabelText("Your name")).toBeTruthy());
  });
});
