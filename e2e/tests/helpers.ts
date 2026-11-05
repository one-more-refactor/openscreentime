import { expect, type Page } from "@playwright/test";

/** True on the phone-sized project, where the rail is a drawer. */
export function isMobile(page: Page): boolean {
  return (page.viewportSize()?.width ?? 1440) < 900;
}

/** The rail: in place on desktop, behind "Menu" on a phone. */
export async function openRail(page: Page) {
  const menu = page.getByRole("button", { name: "Menu", exact: true });
  if (await menu.isVisible()) {
    await menu.click();
    return page.getByRole("dialog", { name: "Menu" });
  }
  return page.locator("body");
}

/** Wait until the console has stopped "Signing in" and drawn a page. */
export async function ready(page: Page) {
  await expect(page.locator('[aria-label="Signing in"]')).toHaveCount(0);
  await expect(page.getByRole("heading", { level: 1 }).first()).toBeVisible();
}
