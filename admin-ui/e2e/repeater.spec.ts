import { expect, signIn, test } from "./fixtures";

// The demo's footer links (ADR 0040): a JSON repeater on the site settings.
// Rows are added, checked in their row, reordered and saved; the links are
// removed again at the end.
test("footer links are added, checked, reordered and saved", async ({ page, watch: _ }) => {
  await signIn(page, "admin@example.test");
  await page.goto("/admin/settings/site");
  await expect(page.getByRole("heading", { name: "Site" })).toBeVisible();
  const links = page.getByRole("group", { name: "Footer links" }).or(page.locator("#field-social_links"));
  await expect(links.getByText("No items yet.")).toBeVisible();

  await page.getByRole("button", { name: "Add a link" }).click();
  await page.getByRole("button", { name: "Add a link" }).click();
  const rows = links.getByRole("listitem");
  await expect(rows).toHaveCount(2);
  await rows.nth(0).getByLabel(/^Label/).fill("Docs");
  await rows.nth(0).getByLabel(/^Url/).fill("https://docs.example.test");
  // The second row has no label yet: the panel says so in the row.
  await rows.nth(1).getByLabel(/^Url/).fill("https://chat.example.test");
  await page.getByRole("button", { name: "Save settings" }).click();
  await expect(rows.nth(1).getByRole("alert")).toHaveText("is required");

  await rows.nth(1).getByLabel(/^Label/).fill("Chat");
  // Each row's heading follows its label; Chat goes first.
  await rows.nth(1).getByRole("button", { name: "Move up" }).click();
  await expect(rows.nth(0).getByText("Chat")).toBeVisible();
  await page.getByRole("button", { name: "Save settings" }).click();
  await expect(page.getByText("Settings saved.")).toBeVisible();

  await page.reload();
  await expect(rows).toHaveCount(2);
  await expect(rows.nth(0).getByLabel(/^Label/)).toHaveValue("Chat");
  await expect(rows.nth(1).getByLabel(/^Url/)).toHaveValue("https://docs.example.test");

  // Leave nothing behind.
  await rows.nth(0).getByRole("button", { name: "Remove" }).click();
  await rows.nth(0).getByRole("button", { name: "Remove" }).click();
  await page.getByRole("button", { name: "Save settings" }).click();
  await expect(page.getByText("Settings saved.")).toBeVisible();
});
