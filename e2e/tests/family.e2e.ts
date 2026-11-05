// The parent's home: the family at a glance, a person's day, their rules.
import { expect, test } from "@playwright/test";
import { openRail, ready } from "./helpers";

test.describe("Family", () => {
  test("opens on the family: a greeting and a card per person", async ({ page }) => {
    await page.goto("/");
    await ready(page);
    await expect(page.getByRole("heading", { level: 1 })).toHaveText(/Good (morning|afternoon|evening)/);
    // The sample household (web/src/mock.ts).
    for (const name of ["Mia", "Noah", "Leo"]) {
      await expect(page.getByText(name, { exact: true }).first()).toBeVisible();
    }
    // Sample data says so, so a screenshot can't be mistaken for a real family.
    const rail = await openRail(page);
    await expect(rail.getByText("Sample data")).toBeVisible();
  });

  test("the rail reaches every place", async ({ page }) => {
    await page.goto("/");
    await ready(page);
    // Me's title is the parent's own computer's name, so only its presence counts.
    const places: Array<[string, RegExp, RegExp | string | null]> = [
      ["Computers", /\/computers$/, "Computers"],
      ["Settings", /\/settings$/, "Settings"],
      ["Me", /\/me$/, null],
      ["Family", /\/$/, /Good (morning|afternoon|evening)/],
    ];
    for (const [label, url, heading] of places) {
      const rail = await openRail(page);
      await rail.getByRole("navigation", { name: "Main" }).getByRole("link", { name: label }).click();
      await expect(page).toHaveURL(url);
      const h1 = page.getByRole("heading", { level: 1 });
      if (heading === null) await expect(h1).toBeVisible();
      else await expect(h1).toHaveText(heading);
    }
  });

  test("a person: today, then their rules", async ({ page }) => {
    await page.goto("/child/acc-mia");
    await ready(page);
    await expect(page.getByRole("heading", { level: 1 })).toHaveText("Mia");
    const tabs = page.getByRole("navigation", { name: "Mia's page" });
    await expect(tabs.getByRole("link", { name: "Today" })).toHaveClass(/active/);

    await tabs.getByRole("link", { name: "Rules" }).click();
    await expect(page).toHaveURL(/\/child\/acc-mia\/rules$/);
    await expect(page.getByText("Daily limit").first()).toBeVisible();
    await expect(page.getByText("Screen time a day, across all their computers.", { exact: false })).toBeVisible();
  });

  test("an unknown person is said plainly, not an error page", async ({ page }) => {
    await page.goto("/child/nobody-here");
    await ready(page);
    await expect(page.getByRole("heading", { level: 1 })).toHaveText("Nobody here");
  });

  test("an unknown address lands on the family", async ({ page }) => {
    await page.goto("/this/does/not/exist");
    await ready(page);
    await expect(page).toHaveURL(/\/$/);
  });

  test("an empty household invites the first person", async ({ page }) => {
    await page.goto("/?mock=empty");
    await ready(page);
    await expect(page.getByText("Add a person").first()).toBeVisible();
  });
});

test.describe("Computers", () => {
  test("lists the household's computers", async ({ page }) => {
    await page.goto("/computers");
    await ready(page);
    await expect(page.getByRole("heading", { level: 1 })).toHaveText("Computers");
    await expect(page.getByText("Living Room PC").first()).toBeVisible();
  });
});
