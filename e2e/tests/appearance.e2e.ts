// Light, dark, or match my system — one choice, remembered in this browser.
import { expect, test } from "@playwright/test";
import { ready } from "./helpers";

test("a new visitor gets the light theme", async ({ page }) => {
  await page.emulateMedia({ colorScheme: "dark" });
  await page.goto("/");
  await ready(page);
  // Deliberately not the OS's dark: a family console starts warm and light.
  await expect(page.locator("html")).toHaveAttribute("data-theme", "light");
});

test("choosing Dark in Settings sticks across a reload", async ({ page }) => {
  await page.goto("/settings");
  await ready(page);
  const group = page.getByRole("radiogroup");
  await group.getByRole("radio", { name: "Dark" }).click();
  await expect(group.getByRole("radio", { name: "Dark" })).toHaveAttribute("aria-checked", "true");
  await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");

  await page.reload();
  await ready(page);
  await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");
});

test("Match my system follows the system", async ({ page }) => {
  await page.emulateMedia({ colorScheme: "dark" });
  await page.goto("/settings");
  await ready(page);
  await page.getByRole("radio", { name: "Match my system" }).click();
  await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");
  await page.emulateMedia({ colorScheme: "light" });
  await expect(page.locator("html")).toHaveAttribute("data-theme", "light");
});
