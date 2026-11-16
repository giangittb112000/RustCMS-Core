import { expect, signIn, test } from "./fixtures";

// Form reactions (ADR 0041): the demo's category name fills an empty slug on
// the server once typing pauses; a slug the user typed is kept. The `watch`
// fixture fails on any console error. Nothing is saved.
test("typing a category's name fills its slug, never over the user's", async ({ page, watch: _ }) => {
  await signIn(page, "admin@example.test");
  await page.goto("/admin/r/categories/create");
  const name = page.getByLabel("Name");
  const slug = page.getByLabel("Slug");
  await name.pressSequentially("Garden Tools & More");
  await expect(slug).toHaveValue("garden-tools-more");

  // Typed by the user: the reaction leaves it alone.
  await slug.fill("mine");
  await name.pressSequentially(" 2");
  // The answer to the last pause has come back (nothing to change).
  const answered = page.waitForResponse((r) => r.url().endsWith("/categories/form/react"));
  await name.press("End");
  await name.pressSequentially("!");
  await answered;
  await expect(slug).toHaveValue("mine");
});
