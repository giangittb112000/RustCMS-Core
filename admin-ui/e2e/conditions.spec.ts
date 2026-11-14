import { expect, signIn, test } from "./fixtures";
import { trashPost } from "./records";

// An input shown under a condition: the post's publication date only while
// "Published" is on. It appears and disappears as the switch changes, keeps
// what was typed, and a date typed then hidden is not saved. Leaves nothing
// behind.
test("the publication date is shown only for published posts", async ({ page, watch: _ }) => {
  const title = `Conditional date ${Date.now().toString(36)}`;
  await signIn(page, "admin@example.test");
  await page.goto("/admin/r/posts/create");
  await page.getByLabel("Title").fill(title);
  await page.getByRole("tab", { name: "Publishing" }).click();

  const published = page.getByRole("switch", { name: "Published" });
  const date = page.getByLabel("Published at");
  await expect(published).not.toBeChecked();
  await expect(date).toHaveCount(0);

  // Shown at once when switched on; a date typed in.
  await published.click();
  await expect(date).toBeVisible();
  await date.fill("2026-03-04T05:06:07");

  // Hidden again, then shown: what was typed is still there.
  await published.click();
  await expect(date).toHaveCount(0);
  await published.click();
  await expect(date).toHaveValue("2026-03-04T05:06:07");

  // Saved unpublished: the hidden date is not sent, so not stored.
  await published.click();
  await page.getByRole("button", { name: "Create" }).click();
  await expect(page.getByRole("heading", { name: title })).toBeVisible();
  const key = page.url().split("/").pop()!;
  const record = (await (await page.request.get(`/admin/api/resources/posts/records/${key}`)).json()) as {
    data: { data: Record<string, unknown> };
  };
  expect(record.data.data.published).toBe(false);
  expect(record.data.data.published_at).toBeNull();

  // The record's page hides it too while unpublished.
  await page.getByRole("main").getByRole("tab", { name: "Publishing" }).click();
  await expect(page.getByRole("main").getByText("Published at")).toHaveCount(0);

  await trashPost(page, key);
});
