// Form reactions (ADR 0041): the state sent to the server, the patches
// applied (in order, rows included), warnings under rows, and the create
// form asking after a pause, applying only the latest answer to an unchanged
// form, ignoring failures. Schemas from the server's contract.
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { RouterProvider, createMemoryHistory } from "@tanstack/react-router";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { http, HttpResponse, delay } from "msw";
import { setupServer } from "msw/node";
import { useForm } from "react-hook-form";
import { SessionContext } from "@/api/context";
import type { ComponentSchema, FormPatch, FormReactRequest, InputSchema, ResourceSchema } from "@/api/types";
import { createAppRouter } from "@/app/router";
import { createSession } from "@/app/session";
import { stayOnPurpose } from "@/lib/leave";
import { Components } from "@/renderers/form";
import { type FormValues, defaultValues, inputsOf } from "@/schema/form";
import { applyPatches, reactionState } from "@/schema/reactions";
import create from "../../fixtures/contract/kitchen-sink.schema.create.json";
import reacted from "../../fixtures/contract/kitchen-sink.react.json";
import products from "../../fixtures/contract/repeater.schema.create.json";

const gadgets = inputsOf((create as unknown as ResourceSchema).form.components);
const productComponents = products.form.components as unknown as ComponentSchema[];
const productInputs = inputsOf(productComponents);

describe("state and patches", () => {
  it("sends wire values, never secrets, links or write-only values", () => {
    const values = defaultValues(gadgets, { name: "Lamp", count: 3, price: "1.50", api_token: { set: true } });
    const state = reactionState(gadgets, { ...values, secret: "hunter2" });
    expect(state).toMatchObject({ name: "Lamp", count: 3, price: "1.50" });
    for (const name of ["secret", "api_token", "env"]) expect(state).not.toHaveProperty(name);
  });

  it("applies patches in order, rows included, and skips what the form does not have", () => {
    const values = defaultValues(productInputs, { name: "Lamp", faq: [{ question: "Size?" }, { question: "Colour?" }] });
    const patches: FormPatch[] = [
      { op: "set_if_empty", field: "name", value: "Ignored" },
      { op: "set", field: "price", value: 12 },
      { op: "set", field: "nope", value: 1 },
      { op: "add_row", field: "faq", index: 0, row: { question: "First?" } },
      { op: "move_row", field: "faq", from: 2, to: 0 },
      { op: "remove_row", field: "faq", index: 1 },
      { op: "remove_row", field: "faq", index: 9 },
      { op: "flag_row", field: "faq", index: 0, message: "Check this" },
      { op: "add_row", field: "price", row: {} },
      { op: "notice", message: "Done" },
    ];
    const applied = applyPatches(productInputs, values, patches);
    expect(Object.keys(applied.values).sort()).toEqual(["faq", "price"]);
    expect(applied.values.price).toBe("12");
    const questions = (JSON.parse(applied.values.faq as string) as { question: string }[]).map((r) => r.question);
    expect(questions).toEqual(["Colour?", "Size?"]);
    expect(applied.flags).toEqual({ faq: { 0: ["Check this"] } });
    expect(applied.notices).toEqual(["Done"]);
    // An empty field takes `set_if_empty`.
    expect(applyPatches(productInputs, defaultValues(productInputs, {}), [patches[0]!]).values).toEqual({ name: "Ignored" });
  });

  it("shows warnings under the rows they name", () => {
    function Harness() {
      const form = useForm<FormValues>({ defaultValues: defaultValues(productInputs, { faq: [{ question: "A" }, { question: "B" }] }) });
      return (
        <Components components={productComponents}
          ctx={{ slug: "products", control: form.control, errors: form.formState.errors, rowFlags: { faq: { 1: ["Needs an answer"] } } }} />
      );
    }
    render(<Harness />);
    const rows = screen.getAllByRole("listitem");
    expect(within(rows[0]!).queryByText("Needs an answer")).toBeNull();
    expect(within(rows[1]!).getByRole("status")).toHaveTextContent("Needs an answer");
  });
});

let requests: FormReactRequest[] = [];
let answer: (request: FormReactRequest) => Promise<Response> = async (request) =>
  HttpResponse.json({ ...reacted, seq: request.seq });

const server = setupServer(
  http.get("*/admin/api/auth/me", () => HttpResponse.json({ user: { id: "1", name: "A", email: "a@b.c" }, guard: "admin", permissions: [], superAdmin: true })),
  http.get("*/admin/api/auth/csrf", () => HttpResponse.json({ csrfToken: "t", userId: "1" })),
  http.get("*/admin/api/panel", () =>
    HttpResponse.json({ brand: "Test", user: { id: "1", name: "Ada", email: "a@b.c" }, navigation: [(create as unknown as ResourceSchema).resource] }),
  ),
  http.get("*/admin/api/resources/gadgets/schema", () => HttpResponse.json(create)),
  http.get("*/admin/api/resources/:slug/fields/:field/options", () => HttpResponse.json({ data: [] })),
  http.post("*/admin/api/resources/gadgets/form/react", async ({ request }) => {
    const body = (await request.json()) as FormReactRequest;
    requests.push(body);
    return answer(body);
  }),
);
beforeAll(() => server.listen({ onUnhandledRequest: "error" }));
afterAll(() => server.close());
beforeEach(() => {
  stayOnPurpose();
  requests = [];
  answer = async (request) => HttpResponse.json({ ...reacted, seq: request.seq });
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

const notes = () => screen.getByLabelText(/^Notes/) as HTMLTextAreaElement;

describe("the create form", () => {
  it("asks once typing pauses and applies the answer", async () => {
    mount("/r/gadgets/create");
    await userEvent.type(await screen.findByLabelText(/^Name/), "Widget");
    await waitFor(() => expect(notes()).toHaveValue("About Widget"));
    // One request for the whole word, with the form's values.
    expect(requests).toHaveLength(1);
    expect(requests[0]).toMatchObject({ operation: "create", changed: "name", state: { name: "Widget" }, seq: 1 });
    expect(requests[0]).not.toHaveProperty("key");
    expect(await screen.findByText("Notes filled")).toBeInTheDocument();
  });

  it("sends the value the input had when the last answer was applied", async () => {
    mount("/r/gadgets/create");
    const name = await screen.findByLabelText(/^Name/);
    await userEvent.type(name, "Widget");
    await waitFor(() => expect(notes()).toHaveValue("About Widget"));
    await userEvent.type(name, " Pro");
    await waitFor(() => expect(requests).toHaveLength(2));
    expect(requests[1]).toMatchObject({ changed: "name", previous: "Widget", state: { name: "Widget Pro" } });
  });

  it("ignores an answer once the user changed the form again", async () => {
    let release!: () => void;
    const held = new Promise<void>((resolve) => (release = resolve));
    answer = async (request) => {
      await held;
      return HttpResponse.json({ ...reacted, seq: request.seq });
    };
    mount("/r/gadgets/create");
    await userEvent.type(await screen.findByLabelText(/^Name/), "Widget");
    await waitFor(() => expect(requests).toHaveLength(1));
    // Typed while the answer is on its way (an input without a reaction: no new request).
    await userEvent.type(screen.getByLabelText(/^Count/), "4");
    release();
    await delay(50);
    expect(notes()).toHaveValue("");
    expect(requests).toHaveLength(1);
  });

  it("applies only the latest answer", async () => {
    answer = async (request) => {
      // The first answer arrives last.
      if (request.seq === 1) await delay(600);
      return HttpResponse.json({ seq: request.seq, patches: [{ op: "set", field: "notes", value: `From ${request.seq}` }] });
    };
    mount("/r/gadgets/create");
    const name = await screen.findByLabelText(/^Name/);
    await userEvent.type(name, "A");
    await waitFor(() => expect(requests).toHaveLength(1));
    await userEvent.type(name, "B");
    await waitFor(() => expect(notes()).toHaveValue("From 2"));
    await delay(700);
    expect(notes()).toHaveValue("From 2");
  });

  it("ignores failures", async () => {
    answer = async () => HttpResponse.json({ message: "Too many requests.", code: "too_many_requests" }, { status: 429 });
    mount("/r/gadgets/create");
    await userEvent.type(await screen.findByLabelText(/^Name/), "Widget");
    await waitFor(() => expect(requests).toHaveLength(1));
    await delay(50);
    expect(notes()).toHaveValue("");
    expect(screen.queryByRole("alert")).toBeNull();
    answer = async (request) => HttpResponse.json({ seq: request.seq, patches: [{ op: "set", field: "notes", value: "x" }], failed: true });
    await userEvent.type(screen.getByLabelText(/^Name/), "s");
    await waitFor(() => expect(requests).toHaveLength(2));
    await delay(50);
    expect(notes()).toHaveValue("");
  });
});

it("asks nothing for inputs without a reaction", async () => {
  mount("/r/gadgets/create");
  await userEvent.type(await screen.findByLabelText(/^Notes/), "Plain");
  await delay(600);
  expect(requests).toHaveLength(0);
  expect((gadgets.find((i) => i.name === "notes") as InputSchema).reacts).toBeUndefined();
});
