import { expect, signIn, test } from "./fixtures";

// Rich text version 3 in the real panel: underline, superscript, and the
// demo's application elements (`app.kbd`, drawn and edited by its React
// extension; `app.note_ref`, a placeholder and a generated form). The CSP
// watcher of `fixtures.ts` fails the test on any violation.
test("the new marks and application elements are inserted, saved and shown", async ({ page, watch: _ }) => {
  await signIn(page, "admin@example.test");
  await page.goto("/admin/r/posts/create");
  await page.getByLabel("Title").fill("Elements post");
  const editor = page.getByRole("textbox", { name: "Content" });
  await editor.click();

  // Underline and superscript, each toggled on then off.
  await page.keyboard.type("E = mc");
  await page.getByRole("button", { name: "Superscript" }).click();
  await page.keyboard.type("2");
  await page.getByRole("button", { name: "Superscript" }).click();
  await page.keyboard.type(", ");
  await page.getByRole("button", { name: "Underline" }).click();
  await page.keyboard.type("press");
  await page.getByRole("button", { name: "Underline" }).click();
  await page.keyboard.type(" ");
  await expect(editor.locator("sup")).toHaveText("2");
  await expect(editor.locator("u")).toHaveText("press");

  // An inline element, edited by the extension's own dialog, drawn by its view.
  await page.getByRole("button", { name: "Insert an element" }).click();
  await page.getByRole("menuitem", { name: "Keyboard shortcut" }).click();
  const shortcut = page.getByRole("dialog", { name: "Keyboard shortcut" });
  await shortcut.getByLabel("Keys").fill("Ctrl+S");
  await shortcut.getByRole("button", { name: "Apply" }).click();
  await expect(editor.locator(".demo-kbd kbd")).toHaveText(["Ctrl", "S"]);

  // A block element without a view: the generated form, then a placeholder.
  await page.getByRole("button", { name: "Insert an element" }).click();
  await page.getByRole("menuitem", { name: "Note reference" }).click();
  const note = page.getByRole("dialog", { name: "Note reference" });
  // Its required number, missing: refused until given.
  await note.getByRole("button", { name: "Apply" }).click();
  await expect(note.getByRole("alert")).toBeVisible();
  await note.getByLabel(/number/).fill("7");
  await note.getByRole("button", { name: "Apply" }).click();
  await expect(editor.getByRole("img", { name: "Note reference" })).toBeVisible();

  expect(await page.locator("[style]").count(), "no inline styles").toBe(0);
  await page.getByRole("button", { name: "Create" }).click();
  await expect(page.getByRole("heading", { name: "Elements post" })).toBeVisible();

  // The view page draws the same document.
  const main = page.getByRole("main");
  await expect(main.locator("sup")).toHaveText("2");
  await expect(main.locator("u")).toHaveText("press");
  await expect(main.locator(".demo-kbd kbd")).toHaveText(["Ctrl", "S"]);
  await expect(main.getByRole("img", { name: "Note reference" })).toBeVisible();

  // The edit form opens it as it is, unchanged: leaving asks nothing.
  await page.getByRole("link", { name: "Edit" }).click();
  await expect(page.getByRole("textbox", { name: "Content" }).locator(".demo-kbd kbd")).toHaveText(["Ctrl", "S"]);
  await page.goto("/admin/r/posts?search=Elements+post");

  // Leave no record behind for the other specs.
  await page.getByRole("row", { name: /Elements post/ }).getByRole("button", { name: "Delete" }).click();
  await page.getByRole("button", { name: "Confirm" }).click();
  await page.getByLabel("Trashed records").selectOption("only");
  await page.getByRole("row", { name: /Elements post/ }).getByRole("button", { name: "Delete permanently" }).click();
  await page.getByRole("button", { name: "Confirm" }).click();
  await expect(page.getByText("Deleted permanently.")).toBeVisible();
});
