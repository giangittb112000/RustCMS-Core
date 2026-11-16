// The default view page (plan branch 4): the infolist the server makes from a
// form draws each value as the form's read-only view did. The two were
// compared value by value before the read-only view was removed; the
// snapshots keep what it showed. Also checks tests/view.ts makes the
// server's entries, for tests written against form schemas.
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { RouterProvider, createMemoryHistory, createRootRoute, createRouter } from "@tanstack/react-router";
import { render, screen } from "@testing-library/react";
import { http, HttpResponse } from "msw";
import { setupServer } from "msw/node";
import { SessionContext } from "@/api/context";
import type { Data, Envelope, ResourceSchema } from "@/api/types";
import { createSession } from "@/app/session";
import { InfolistRenderer } from "@/renderers/infolist";
import gadgetSchema from "../../fixtures/contract/kitchen-sink.schema.view-record.json";
import gadgets from "../../fixtures/contract/kitchen-sink.records.json";
import lessonSchema from "../../fixtures/contract/view-page.schema.view.json";
import lesson from "../../fixtures/contract/view-page.record.json";
import { viewOf } from "./view";

// Thumbnails ask for their media; none here.
const server = setupServer(http.get("*", () => HttpResponse.json({}, { status: 404 })));
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
    const dd = (dl.querySelector(":scope > dd") as HTMLElement).cloneNode(true) as HTMLElement;
    // Instants read in the machine's time zone: kept as their wire value, so snapshots hold anywhere.
    for (const time of dd.querySelectorAll("time")) {
      const at = time.getAttribute("dateTime") ?? "";
      if (/(Z|[+-]\d\d:\d\d)$/.test(at)) time.textContent = `[instant ${at}]`;
    }
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

async function draw(schema: ResourceSchema, record: Envelope, slug: string) {
  const view = await mount(<InfolistRenderer infolist={schema.infolist!} form={schema.form.components} data={record.data as Data} labels={record.labels} slug={slug} recordKey={record.key} />);
  return shown(view.container);
}

/** Without `link`: whether a relation links depends on the user, not on the form. */
const unlinked = (components: unknown) => JSON.parse(JSON.stringify(components, (k, v) => (k === "link" ? undefined : v)));

const gadget = (gadgets as unknown as { data: Envelope[] }).data.find((r) => r.data.name === "Widget")!;
const cases: [string, ResourceSchema, Envelope, string][] = [
  ["a kitchen sink record", gadgetSchema as unknown as ResourceSchema, gadget, "gadgets"],
  ["repeaters, rich text, masks and conditions", lessonSchema as unknown as ResourceSchema, lesson as unknown as Envelope, "lessons"],
];

it.each(cases)("draws %s as the form's read-only view did", async (_, schema, record, slug) => {
  expect(await draw(schema, record, slug)).toMatchSnapshot();
});

it.each(cases)("makes the server's entries from the form of %s (tests/view.ts)", (_, schema) => {
  expect(unlinked(viewOf(schema.form.components))).toEqual(unlinked(schema.infolist!.components));
});
