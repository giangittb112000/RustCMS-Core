import { expect, signIn, test } from "./fixtures";
import { adminApi } from "./records";

const PNG = Buffer.from(
  "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNkYAAAAAYAAjCB0C8AAAAASUVORK5CYII=",
  "base64",
);

// View pages without `infolist()`: drawn from the form (a post, a tag, a file of
// the media library, an integration with encrypted fields). The `watch` fixture
// fails on any console error. Leaves nothing behind.
test("default view pages show each value as the form did, secrets never", async ({ page, watch: _ }) => {
  await signIn(page, "admin@example.test");

  // A post with a cover and a category: a thumbnail, and the category as a link.
  await page.goto("/admin/r/posts/create");
  await page.getByLabel("Title").fill("Viewed post");
  await page.locator("input[type=file]").first().setInputFiles({ name: "viewed-cover.png", mimeType: "image/png", buffer: PNG });
  await expect(page.getByRole("img", { name: "viewed-cover.png" })).toBeVisible();
  await page.getByRole("button", { name: "Create" }).click();
  await expect(page.getByRole("heading", { name: "Viewed post" })).toBeVisible();
  const key = page.url().split("/").pop()!;
  const categories = (await (await page.request.get("/admin/api/resources/categories/records")).json()) as {
    data: { key: string; data: { name: string } }[];
  };
  const category = categories.data[0]!;
  await adminApi(page, "PATCH", `resources/posts/records/${key}`, { category_id: Number(category.key) });
  await page.goto(`/admin/r/posts/records/${key}`);
  // The form's tabs: the cover in "Content", the category in "Publishing".
  const content = page.getByRole("tabpanel", { name: "Content" });
  await expect(content.locator("dl").filter({ hasText: "Cover" }).locator("dd img")).toBeVisible();
  await page.getByRole("tab", { name: "Publishing" }).click();
  const link = page.getByRole("tabpanel", { name: "Publishing" }).getByRole("link", { name: category.data.name });
  await expect(link).toHaveAttribute("href", `/admin/r/categories/records/${category.key}`);
  const post = (await (await page.request.get(`/admin/api/resources/posts/records/${key}`)).json()) as { data: { data: { cover: string } } };

  // The file in the media library.
  await page.goto(`/admin/r/media/records/${post.data.data.cover}`);
  await expect(page.getByRole("main").getByText("viewed-cover.png").first()).toBeVisible();

  // A tag.
  const tags = (await (await page.request.get("/admin/api/resources/tags/records")).json()) as { data: { key: string; data: { name: string } }[] };
  await page.goto(`/admin/r/tags/records/${encodeURIComponent(tags.data[0]!.key)}`);
  await expect(page.getByRole("main").locator("dd").getByText(tags.data[0]!.data.name)).toBeVisible();

  // An integration: its name, and no entry for its encrypted fields.
  await page.goto("/admin/r/integrations/create");
  await page.getByLabel("Name").fill("Viewed integration");
  await page.getByRole("button", { name: "Create" }).click();
  await expect(page.getByRole("heading", { name: "Viewed integration" })).toBeVisible();
  const integration = page.url().split("/").pop()!;
  const main = page.getByRole("main");
  await expect(main.locator("dd").getByText("Viewed integration")).toBeVisible();
  await expect(main.locator("dt").filter({ hasText: /^(API token|Environment)$/ })).toHaveCount(0);

  await adminApi(page, "DELETE", `resources/integrations/records/${integration}`);
  await adminApi(page, "DELETE", `resources/posts/records/${key}`);
  await adminApi(page, "DELETE", `resources/media/records/${post.data.data.cover}`);
});
