import type { Page } from "@playwright/test";
import { expect, signIn, test } from "./fixtures";

// The demo's categories edit their posts in the form (ADR 0040, part 2): a
// relationship repeater whose rows are posts. Rows are added on create,
// checked in their row, renamed and removed on edit; the categories list
// counts the posts (part 3). Everything is removed again at the end.

/** Deletes a trashed post for good (rows removed in the form go to the trash). */
async function purge(page: Page, title: string) {
  await page.goto(`/admin/r/posts?search=${encodeURIComponent(title)}`);
  await page.getByLabel("Trashed records").selectOption("only");
  await page.getByRole("row", { name: title }).getByRole("button", { name: "Delete permanently" }).click();
  await page.getByRole("button", { name: "Confirm" }).click();
  await expect(page.getByText("Deleted permanently.")).toBeVisible();
}

test("a category's posts are added, checked, renamed and removed in its form", async ({ page, watch: _ }) => {
  // Names of this run: a retry never meets the records of a failed attempt.
  const run = Date.now().toString(36);
  const name = `E2E rows ${run}`;
  const [first, second, renamed] = [`Row one ${run}`, `Row two ${run}`, `Row renamed ${run}`];
  await signIn(page, "admin@example.test");

  // Create: the category and two posts in one save.
  await page.goto("/admin/r/categories/create");
  await page.getByLabel("Name").fill(name);
  await page.getByLabel("Slug").fill(`e2e-rows-${run}`);
  // The only list of the form: the repeater's rows.
  const rows = page.getByRole("main").getByRole("listitem");
  await page.getByRole("button", { name: "Add a post" }).click();
  await page.getByRole("button", { name: "Add a post" }).click();
  await expect(rows).toHaveCount(2);
  await rows.nth(0).getByLabel(/^Title/).fill(first);
  // The second row has no title yet: the server's answer lands in its row.
  await page.getByRole("button", { name: "Create" }).click();
  await expect(rows.nth(1).getByRole("alert")).toHaveText("is required");
  await rows.nth(1).getByLabel(/^Title/).fill(second);
  await page.getByRole("button", { name: "Create" }).click();
  await expect(page.getByRole("heading", { name })).toBeVisible();
  const url = page.url();
  const related = page.getByRole("table", { name: "Posts" });
  await expect(related.getByText(first)).toBeVisible();
  await expect(related.getByText(second)).toBeVisible();

  // The list counts them.
  await page.goto(`/admin/r/categories?search=${encodeURIComponent(name)}`);
  await expect(page.getByRole("columnheader", { name: "Posts" })).toBeVisible();
  await expect(page.getByRole("row", { name }).getByRole("cell", { name: "2", exact: true })).toBeVisible();

  // Edit: rename the first, remove the second; only that change is sent.
  await page.goto(`${url}/edit`);
  await expect(rows).toHaveCount(2);
  await expect(rows.nth(0).getByLabel(/^Title/)).toHaveValue(first);
  await rows.nth(0).getByLabel(/^Title/).fill(renamed);
  await rows.nth(1).getByRole("button", { name: "Remove" }).click();
  await expect(rows).toHaveCount(1);
  await page.getByRole("button", { name: "Save" }).click();
  await expect(page).toHaveURL(url);
  await expect(related.getByText(renamed)).toBeVisible();
  await expect(related.getByText(second)).toHaveCount(0);
  await page.goto(`/admin/r/categories?search=${encodeURIComponent(name)}`);
  await expect(page.getByRole("row", { name }).getByRole("cell", { name: "1", exact: true })).toBeVisible();

  // Leave nothing behind: the last row, both posts for good, the category.
  await page.goto(`${url}/edit`);
  await rows.nth(0).getByRole("button", { name: "Remove" }).click();
  await page.getByRole("button", { name: "Save" }).click();
  await expect(page).toHaveURL(url);
  await purge(page, renamed);
  await purge(page, second);
  await page.goto(`/admin/r/categories?search=${encodeURIComponent(name)}`);
  await page.getByRole("row", { name }).getByRole("button", { name: "Delete" }).click();
  await page.getByRole("button", { name: "Confirm" }).click();
  await expect(page.getByRole("row", { name })).toHaveCount(0);
});
