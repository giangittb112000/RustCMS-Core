// The default view page (plan branch 4): the infolist the server makes from a
// form draws each value as the form's read-only view did. Both views drawn
// from the same contract fixtures, compared value by value.
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { RouterProvider, createMemoryHistory, createRootRoute, createRouter } from "@tanstack/react-router";
import { render, screen, waitFor } from "@testing-library/react";
import { http, HttpResponse } from "msw";
import { setupServer } from "msw/node";
import { SessionContext } from "@/api/context";
import type { Data, Envelope, ResourceSchema } from "@/api/types";
import { createSession } from "@/app/session";
import { ReadOnlyComponents } from "@/renderers/form";
import { InfolistRenderer } from "@/renderers/infolist";
import gadgetSchema from "../../fixtures/contract/kitchen-sink.schema.view-record.json";
import gadgets from "../../fixtures/contract/kitchen-sink.records.json";
import lessonSchema from "../../fixtures/contract/view-page.schema.view.json";
import lesson from "../../fixtures/contract/view-page.record.json";

// The old view asks the options endpoint for a relation's title; the envelope carries the same title.
let titles: Record<string, string> = {};
const server = setupServer(
  http.get("*/admin/api/resources/:slug/fields/:field/options", ({ request }) => {
    const key = new URL(request.url).searchParams.get("selected") ?? "";
    return HttpResponse.json({ data: Object.entries(titles).map(([value, label]) => ({ value: JSON.parse(value), label })).filter((o) => key === "" || JSON.stringify(o.value) === key || String(o.value) === key) });
  }),
  http.get("*", () => HttpResponse.json({}, { status: 404 })),
);
beforeAll(() => server.listen({ onUnhandledRequest: "bypass" }));
afterAll(() => server.close());
afterEach(() => server.resetHandlers());

async function mount(node: React.ReactNode) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const session = createSession({ basePath: "/admin", apiPath: "http://localhost/admin/api" }, client, () => {});
  // Relation entries link to the related record (a router is needed for links).
  const router = createRouter({
    routeTree: createRootRoute({ component: () => <div data-testid="view" className="grid">{node}</div> }),
    history: createMemoryHistory({ initialEntries: ["/"] }),
  });
  const view = render(<SessionContext.Provider value={session}><QueryClientProvider client={client}>
    <RouterProvider router={router} />
  </QueryClientProvider></SessionContext.Provider>);
  await screen.findByTestId("view");
  return view;
}

/** Each shown value: its label, text, width, and how it is marked up. */
function shown(root: HTMLElement) {
  return [...root.querySelectorAll("dl")].filter((dl) => dl.querySelector(":scope > dt")).map((dl) => {
    const dd = dl.querySelector(":scope > dd") as HTMLElement;
    return {
      label: dl.querySelector(":scope > dt")!.textContent,
      text: dd.textContent,
      width: dl.className,
      // Line breaks kept where there are some (D1), dates as <time> (D3), thumbnails.
      preWrap: dd.textContent!.includes("\n")
        ? dd.classList.contains("whitespace-pre-wrap") || !!dd.querySelector(".whitespace-pre-wrap")
        : null,
      time: dd.querySelector("time")?.getAttribute("dateTime") ?? null,
      images: dd.querySelectorAll("img, [data-media]").length,
    };
  });
}

async function both(schema: ResourceSchema, record: Envelope, slug: string) {
  titles = Object.fromEntries(Object.entries(record.labels ?? {}).map(([field, label]) => [JSON.stringify(record.data[field]), label]));
  const data = record.data as Data;
  const old = await mount(<ReadOnlyComponents components={schema.form.components} data={data} slug={slug} record={record.key} />);
  // The old view's relation titles arrive after a request.
  await waitFor(() => expect(old.container.textContent).not.toMatch(/Loading/));
  const before = shown(old.container);
  old.unmount();
  const made = await mount(<InfolistRenderer infolist={schema.infolist!} data={data} labels={record.labels} slug={slug} recordKey={record.key} />);
  const after = shown(made.container);
  made.unmount();
  return { before, after };
}

it("draws a kitchen sink record as the form's read-only view did", async () => {
  const record = (gadgets as unknown as { data: Envelope[] }).data.find((r) => r.data.name === "Widget")!;
  const { before, after } = await both(gadgetSchema as unknown as ResourceSchema, record, "gadgets");
  expect(after).toEqual(before);
});

it("draws repeaters, rich text, masks and conditions as the form's read-only view did", async () => {
  const { before, after } = await both(lessonSchema as unknown as ResourceSchema, lesson as unknown as Envelope, "lessons");
  expect(after).toEqual(before);
});
