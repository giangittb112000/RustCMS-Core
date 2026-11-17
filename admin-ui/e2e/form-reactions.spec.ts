import { expect, signIn, test } from "./fixtures";

// Form reactions (ADR 0043): the demo's category slug follows its name on the
// server once typing pauses, also across pauses ("Garden", then " Tools"),
// until the user edits the slug. The `watch` fixture fails on any console
// error. Nothing is saved.
test("a category's slug follows its name across pauses, never over the user's", async ({ page, watch: _ }) => {
  await signIn(page, "admin@example.test");
  await page.goto("/admin/r/categories/create");
  const name = page.getByLabel("Name");
  const slug = page.getByLabel("Slug");
  await name.pressSequentially("Garden");
  await expect(slug).toHaveValue("garden");
  // A pause, then more of the name: the slug still follows.
  await name.pressSequentially(" Tools & More");
  await expect(slug).toHaveValue("garden-tools-more");

  // Typed by the user: the reaction leaves it alone.
  await slug.fill("mine");
  const answered = page.waitForResponse((r) => r.url().endsWith("/categories/form/react"));
  await name.press("End");
  await name.pressSequentially(" 2");
  await answered;
  await expect(slug).toHaveValue("mine");
});
