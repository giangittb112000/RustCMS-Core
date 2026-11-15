import { expect, signIn, test } from "./fixtures";

// The demo's category view page (ADR 0039): its infolist replaces the form,
// a computed value (the number of posts) loads alongside, and the color is
// drawn by the demo's `demo/swatch` entry (a v2 script). Nothing is saved.
test("a category's view page shows its infolist, a computed value and a custom entry", async ({ page, watch: _ }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await signIn(page, "admin@example.test");
  await page.goto("/admin/r/categories");
  const row = page.getByRole("row").filter({ has: page.getByRole("cell", { name: "News", exact: true }) });
  await row.getByRole("link", { name: "View" }).click();
  const main = page.getByRole("main");
  await expect(main.getByRole("heading", { name: "News", level: 1 })).toBeVisible();

  const value = (label: string) => main.locator("dt", { hasText: new RegExp(`^${label}$`) }).locator("xpath=following-sibling::dd[1]");
  // Text entries: a copy button, a prefix; no inputs.
  await expect(value("Name")).toContainText("News");
  await expect(value("Name").getByRole("button", { name: "Copy" })).toBeVisible();
  await expect(value("Slug")).toHaveText("/categories/news");
  // (The only text box is the relation manager's search.)
  await expect(main.getByRole("textbox", { name: "Name" })).toHaveCount(0);

  // Computed on the server, after the page: a number.
  await expect(value("Posts")).toHaveText(/^\d+$/);

  // The aside: drawn by the extension's entry (the seeded categories have no color).
  const aside = main.locator("aside");
  await expect(aside.getByText("Display")).toBeVisible();
  await expect(value("Color")).toHaveText("No color");
  await expect(value("Color")).not.toHaveAttribute("data-fallback", "true");

  // Around the body nothing changed: the relation manager still lists the posts.
  await expect(main.getByRole("heading", { name: "Posts" })).toBeVisible();
});
