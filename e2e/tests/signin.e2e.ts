// The two-door sign-in, door one: a name, then the code shown on your own
// computer. In mock mode the code is 123456 (web/src/mock.ts MOCK_CODE).
import { expect, test } from "@playwright/test";
import { openRail, ready } from "./helpers";

test("sign out, then back in with the code from your computer", async ({ page }) => {
  await page.goto("/");
  await ready(page);

  const rail = await openRail(page);
  await rail.getByRole("button", { name: "Sign out" }).click();
  await expect(page).toHaveURL(/\/login$/);
  await expect(page.getByRole("heading", { level: 1 })).toHaveText("Sign in");

  // Continue stays disabled until there is a name.
  const cont = page.getByRole("button", { name: "Continue" });
  await expect(cont).toBeDisabled();
  await page.getByLabel("Your name").fill("philip");
  await cont.click();

  await expect(page.getByRole("heading", { level: 1 })).toHaveText("Check your computer");
  const code = page.getByLabel("The code from your computer");

  // A wrong code is said plainly and the boxes clear.
  await code.fill("000000");
  await expect(page.getByRole("alert")).toContainText("didn't match");
  await expect(code).toHaveValue("");

  // The right one signs in and lands on the family.
  await code.fill("123456");
  await expect(page).toHaveURL(/\/$/);
  await expect(page.getByRole("heading", { level: 1 })).toHaveText(/Good (morning|afternoon|evening)/);
});

test("the sign-in page offers a passkey as the other door", async ({ page }) => {
  await page.goto("/login");
  await expect(page.getByRole("heading", { level: 1 })).toHaveText("Sign in");
  await expect(page.getByRole("button", { name: /passkey/i })).toBeVisible();
});

test("first run asks for a name, then a passkey", async ({ page }) => {
  await page.goto("/login?mock=firstrun");
  await expect(page.getByRole("heading", { level: 1 })).toHaveText("Create your household");
  await expect(page.getByRole("button", { name: "Create passkey" })).toBeDisabled();
});
