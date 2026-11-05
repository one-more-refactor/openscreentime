// A child's own session has exactly one page: their own. The server enforces
// that (every other /api route is 403 for a member); the console keeps the
// address honest too.
import { expect, test } from "@playwright/test";
import { ready } from "./helpers";

test("a child sees their own day", async ({ page }) => {
  await page.goto("/me?as=mia");
  await ready(page);
  await expect(page.getByRole("heading", { level: 1 })).toHaveText("Hi, Mia");
});

for (const path of ["/", "/computers", "/settings", "/child/acc-noah"]) {
  test(`a child opening ${path} lands on their own page`, async ({ page }) => {
    await page.goto(`${path}${path.includes("?") ? "&" : "?"}as=mia`);
    await ready(page);
    await expect(page).toHaveURL(/\/me(\?|$)/);
    await expect(page.getByRole("heading", { level: 1 })).toHaveText("Hi, Mia");
    // No parent controls leak onto a child's page.
    await expect(page.getByRole("button", { name: "Pause everything", exact: true })).toHaveCount(0);
    await expect(page.getByRole("link", { name: "Computers", exact: true })).toHaveCount(0);
  });
}
