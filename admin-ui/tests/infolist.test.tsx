// Infolists (ADR 0039): the view page draws a declared infolist from the
// server's contract (fixtures/contract/infolist.*), loads computed values
// alongside, and shows a failed one as not available; entries draw their
// options; panel pages and the SDK draw entries over plain data.
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { RouterProvider, createMemoryHistory } from "@tanstack/react-router";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { http, HttpResponse, delay } from "msw";
import { setupServer } from "msw/node";
import { SessionContext } from "@/api/context";
import type { EntrySchema, InfolistSchema } from "@/api/types";
import { createAppRouter } from "@/app/router";
import { createSession } from "@/app/session";
import { registerEntry, resetWidgets } from "@/lib/widgets";
import { InfolistRenderer } from "@/renderers/infolist";
import schema from "../../fixtures/contract/infolist.schema.view.json";
import record from "../../fixtures/contract/infolist.record.json";
import computed from "../../fixtures/contract/infolist.computed.json";

let computedCalls = 0;
let computedReply: () => Promise<Response> | Response = () => HttpResponse.json(computed);
const server = setupServer(
  http.get("*/admin/api/auth/me", () => HttpResponse.json({ user: { id: "1", name: "A", email: "a@b.c" }, guard: "admin", permissions: [], superAdmin: true })),
  http.get("*/admin/api/auth/csrf", () => HttpResponse.json({ csrfToken: "t", userId: "1" })),
  http.get("*/admin/api/panel", () => HttpResponse.json({ brand: "T", user: { id: "1", name: "A", email: "a@b.c" }, navigation: [schema.resource] })),
  http.get("*/admin/api/resources/courses/schema", () => HttpResponse.json(schema)),
  http.get("*/admin/api/resources/courses/records/1", () => HttpResponse.json(record)),
  http.get("*/admin/api/resources/courses/records/1/computed", () => {
    computedCalls++;
    return computedReply();
  }),
);
beforeAll(() => server.listen({ onUnhandledRequest: "error" }));
afterAll(() => server.close());
afterEach(() => {
  server.resetHandlers();
  computedCalls = 0;
  computedReply = () => HttpResponse.json(computed);
  resetWidgets();
});

function mount(path: string) {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const session = createSession({ basePath: "", apiPath: "http://localhost/admin/api" }, queryClient, () => {});
  const router = createAppRouter({ queryClient, session }, createMemoryHistory({ initialEntries: [path] }));
  render(
    <SessionContext.Provider value={session}>
      <QueryClientProvider client={queryClient}>
        <RouterProvider router={router} />
      </QueryClientProvider>
    </SessionContext.Provider>,
  );
}

/** The value shown under the label `label`. */
const valueOf = (label: string) => {
  const term = screen.getByText(label, { selector: "dt" });
  return term.nextElementSibling as HTMLElement;
};

it("draws the declared infolist instead of the form, with computed values", async () => {
  mount("/r/courses/records/1");
  await screen.findByText("Algebra", { selector: "h1" });
  // Sections of the infolist; the form's inputs are not drawn.
  expect(screen.getByText("Course")).toBeInTheDocument();
  expect(screen.getByText("Money")).toBeInTheDocument();
  expect(screen.queryByRole("textbox")).toBeNull();

  expect(within(valueOf("Title")).getByText("Algebra")).toBeInTheDocument();
  expect(within(valueOf("Title")).getByRole("button", { name: "Copy" })).toBeInTheDocument();
  // A badge with its option label.
  expect(within(valueOf("Status")).getByText("Open")).toBeInTheDocument();
  expect(valueOf("Published")).toHaveTextContent("Live");
  // The related record's title from the envelope, linked to its page.
  const teacher = within(valueOf("Teacher")).getByRole("link", { name: "Ada" });
  expect(teacher).toHaveAttribute("href", "/r/teachers/records/1");
  expect(valueOf("Fee")).toHaveTextContent("50 VND");
  // Shown: published is true.
  expect(within(valueOf("Meta")).getByText("room")).toBeInTheDocument();
  // No script draws `acme/code`: the value as text.
  expect(valueOf("Code")).toHaveTextContent("ALG-1");

  // Computed values: loaded once, failures not available.
  await waitFor(() => expect(valueOf("Learners")).toHaveTextContent("42"));
  expect(valueOf("Revenue")).toHaveTextContent("500.00");
  for (const name of ["Broken", "Slow", "Huge"]) expect(valueOf(name)).toHaveTextContent("Not available");
  expect(computedCalls).toBe(1);
});

it("keeps the page when computed values cannot be loaded", async () => {
  computedReply = async () => {
    await delay(20);
    return HttpResponse.json({ message: "Server error." }, { status: 500 });
  };
  mount("/r/courses/records/1");
  await screen.findByText("Algebra", { selector: "h1" });
  expect(within(valueOf("Learners")).getByRole("status")).toBeInTheDocument();
  await waitFor(() => expect(valueOf("Learners")).toHaveTextContent("Not available"));
  expect(within(valueOf("Title")).getByText("Algebra")).toBeInTheDocument();
});

const entry = (name: string, extra: Partial<EntrySchema> = {}): EntrySchema => ({
  type: "text_entry", name, label: name, valueType: "string", columnSpan: 1, ...extra,
});
const list = (...components: EntrySchema[]): InfolistSchema => ({ columns: 1, components });

function draw(infolist: InfolistSchema, data: Record<string, unknown>) {
  const queryClient = new QueryClient();
  const session = createSession({ basePath: "", apiPath: "http://localhost/admin/api" }, queryClient, () => {});
  return render(
    <SessionContext.Provider value={session}>
      <QueryClientProvider client={queryClient}>
        <InfolistRenderer infolist={infolist} data={data} />
      </QueryClientProvider>
    </SessionContext.Provider>,
  );
}

it("draws each option of text entries", () => {
  draw(list(
    entry("bio", { limit: 5 }),
    entry("site", { url: true }),
    entry("unsafe", { url: true }),
    entry("price", { valueType: "decimal", decimals: 2, prefix: "$" }),
    entry("tags", { valueType: "json", list: true, options: [{ value: "a", label: "Alpha" }] }),
    entry("pin", { password: true }),
    entry("day", { valueType: "date" }),
    entry("empty"),
    entry("nothing"),
  ), {
    bio: "Long biography", site: "https://example.com", unsafe: "javascript:alert(1)", price: "1234.5",
    tags: ["a", "b"], pin: "1234", day: "2026-10-12", empty: "", nothing: null,
  });
  expect(valueOf("bio")).toHaveTextContent("Long …");
  expect(within(valueOf("bio")).getByTitle("Long biography")).toBeInTheDocument();
  expect(within(valueOf("site")).getByRole("link")).toHaveAttribute("rel", "noopener noreferrer");
  // Only http(s) links.
  expect(within(valueOf("unsafe")).queryByRole("link")).toBeNull();
  expect(valueOf("price")).toHaveTextContent("$1,234.50");
  expect(within(valueOf("tags")).getAllByRole("listitem").map((li) => li.textContent)).toEqual(["Alpha", "b"]);
  expect(valueOf("pin")).not.toHaveTextContent("1234");
  expect(valueOf("day")).not.toHaveTextContent("2026-10-12T");
  expect(valueOf("empty")).toHaveTextContent("Empty string");
  expect(valueOf("nothing")).toHaveTextContent("Not set");
});

it("evaluates conditions on the stored values, layouts included", () => {
  draw({
    columns: 1,
    components: [
      entry("published", { type: "boolean_entry", valueType: "bool" }),
      entry("published_at", { shownWhen: { op: "is_true", name: "published" } }),
      { type: "section", label: "Draft notes", columns: 1, shownWhen: { op: "ne", name: "published", value: true }, components: [entry("notes")] },
    ],
  }, { published: false, published_at: "never", notes: "todo" });
  expect(screen.queryByText("published_at")).toBeNull();
  expect(screen.getByText("Draft notes")).toBeInTheDocument();
  expect(valueOf("published")).toHaveTextContent("No");
});

it("draws custom entries with their script, and unknown entries as text", () => {
  registerEntry("acme/stars", {
    apiVersion: 1,
    mount(element, ctx) {
      element.textContent = "★".repeat(Number(ctx.data));
      return { update: () => {}, unmount: () => {} };
    },
  });
  draw(list(
    entry("rating", { type: "custom_entry", valueType: "i32", widget: "acme/stars" }),
    entry("future", { type: "chart_entry" as EntrySchema["type"] }),
  ), { rating: 3, future: "raw" });
  expect(valueOf("rating")).toHaveTextContent("★★★");
  expect(valueOf("future")).toHaveTextContent("raw");
  expect(valueOf("future")).toHaveAttribute("data-fallback", "true");
});

it("keeps the open tab when the stored values show a tab before it", () => {
  const infolist: InfolistSchema = {
    columns: 1,
    components: [
      {
        type: "tabs", columns: 1, components: [
          { type: "tab", id: "sale", label: "Sale", columns: 1, shownWhen: { op: "is_true", name: "on_sale" }, components: [entry("discount")] },
          { type: "tab", id: "main", label: "Main", columns: 1, components: [entry("title")] },
          { type: "tab", id: "more", label: "More", columns: 1, components: [entry("note")] },
        ],
      },
    ],
  } as InfolistSchema;
  const view = draw(infolist, { on_sale: false, title: "T", note: "N", discount: 5 });
  fireEvent.click(screen.getByRole("tab", { name: "More" }));
  const queryClient = new QueryClient();
  const session = createSession({ basePath: "", apiPath: "http://localhost/admin/api" }, queryClient, () => {});
  view.rerender(
    <SessionContext.Provider value={session}>
      <QueryClientProvider client={queryClient}>
        <InfolistRenderer infolist={infolist} data={{ on_sale: true, title: "T", note: "N", discount: 5 }} />
      </QueryClientProvider>
    </SessionContext.Provider>,
  );
  expect(screen.getByRole("tab", { name: "More" })).toHaveAttribute("aria-selected", "true");
});

it("draws a repeatable entry's rows from the stored list, not as a layout", () => {
  draw({
    columns: 1,
    components: [
      entry("faq", {
        type: "repeatable_entry", valueType: "json",
        components: [entry("question"), entry("answer")],
      } as Partial<EntrySchema>),
    ],
  }, { faq: [{ question: "Size?", answer: "M" }, { question: "Color?", answer: "Red" }] });
  const rows = within(valueOf("faq")).getAllByRole("listitem");
  expect(rows).toHaveLength(2);
  expect(rows[0]).toHaveTextContent("Size?");
  expect(rows[1]).toHaveTextContent("Red");
});

it("draws a repeatable entry's rows from each row, not as a layout", () => {
  const faq = entry("faq", {
    type: "repeatable_entry", valueType: "json",
    components: [entry("question"), entry("asked_on", { valueType: "date" })],
  });
  const { container } = draw(list(faq), { faq: [{ question: "Why?", asked_on: "2026-11-16" }, { question: "How?" }] });
  const rows = container.querySelectorAll("ol > li");
  expect(rows).toHaveLength(2);
  expect(within(rows[0] as HTMLElement).getByText("Why?")).toBeInTheDocument();
  expect(rows[0]!.querySelector("time")?.getAttribute("dateTime")).toBe("2026-11-16");
  expect(within(rows[1] as HTMLElement).getByText("How?")).toBeInTheDocument();
});

it("keeps line breaks only where asked, and in values shown as raw JSON", () => {
  const faq = entry("faq", { type: "repeatable_entry", valueType: "json", components: [entry("question")] });
  const { container } = draw(
    list(faq, entry("note", { multiline: true }), entry("flag", { type: "boolean_entry", valueType: "bool" })),
    { faq: [{ question: "One\nTwo" }], note: "A\nB", flag: { odd: 1 } },
  );
  // A row's text entry without `multiline` does not inherit its row's breaks.
  expect(screen.getByText("One Two", { normalizer: (t) => t.replace(/\s+/g, " ") }).closest(".whitespace-pre-wrap")).toBeNull();
  expect(screen.getByText(/A\s+B/).closest(".whitespace-pre-wrap")).not.toBeNull();
  // A boolean entry over a value that is not one: indented JSON, breaks kept.
  expect(container.querySelector("pre, .whitespace-pre-wrap")?.textContent).toBeTruthy();
  expect(screen.getByText(/"odd": 1/).closest(".whitespace-pre-wrap")).not.toBeNull();
});

it("marks badge dates with <time>", () => {
  const { container } = draw(list(entry("day", { valueType: "date", badge: true })), { day: "2026-11-16" });
  expect(container.querySelector("time")?.getAttribute("dateTime")).toBe("2026-11-16");
});
