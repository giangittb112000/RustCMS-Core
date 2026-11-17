// Relationship repeaters (ADR 0040), from the server's contract
// (fixtures/contract/repeater-relation.*): the stored rows are read from the
// relation's list, in the order column's order; a save sends only the change.
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, fireEvent, render, screen, within } from "@testing-library/react";
import { http, HttpResponse } from "msw";
import { setupServer } from "msw/node";
import { useForm } from "react-hook-form";
import { SessionContext } from "@/api/context";
import type { ComponentSchema, InputSchema, RepeaterSchema } from "@/api/types";
import { createSession } from "@/app/session";
import { Components } from "@/renderers/form";
import { relationRowProblems } from "@/schema/relation-rows";
import { type FormValues, defaultValues, inputsOf, payloadOf } from "@/schema/form";
import schema from "../../fixtures/contract/repeater-relation.schema.edit.json";
import rows from "../../fixtures/contract/repeater-relation.rows.json";

const server = setupServer(
  http.get("*/admin/api/resources/orders/records/:key/relations/lines", () =>
    // Shuffled: the widget orders the rows by their position.
    HttpResponse.json({ ...rows, data: [...rows.data].reverse() })),
);
beforeAll(() => server.listen({ onUnhandledRequest: "error" }));
afterAll(() => server.close());

const components = schema.form.components as unknown as ComponentSchema[];
const inputs = inputsOf(components);

let form: ReturnType<typeof useForm<FormValues>> | undefined;
function Harness() {
  form = useForm<FormValues>({ defaultValues: defaultValues(inputs, { number: "A-1" }) });
  return <Components components={components} ctx={{ slug: "orders", record: "1", control: form.control, errors: form.formState.errors }} />;
}

function mount() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const session = createSession({ basePath: "", apiPath: "http://localhost/admin/api" }, client, () => {});
  render(<SessionContext.Provider value={session}><QueryClientProvider client={client}><Harness /></QueryClientProvider></SessionContext.Provider>);
}

const rowsShown = () => screen.getAllByRole("listitem");
const sent = () => payloadOf(inputs, form!.getValues()).lines;

it("reads the stored rows in order and sends nothing until they change", async () => {
  mount();
  await screen.findByText("a");
  expect(rowsShown().map((li) => within(li).getByLabelText(/^Product/).getAttribute("value"))).toEqual(["a", "b", "c"]);
  expect(sent()).toBeUndefined();
});

it("sends the order only when the user moved rows", async () => {
  mount();
  await screen.findByText("a");
  fireEvent.change(within(rowsShown()[0]!).getByLabelText(/^Quantity/), { target: { value: "5" } });
  // A row another user added meanwhile cannot make this change incomplete.
  expect(sent()).toEqual({ update: [{ key: "1", data: { quantity: 5 } }] });
});

it("sends only the change: updated fields, removed rows, new rows and the order", async () => {
  mount();
  await screen.findByText("a");
  fireEvent.change(within(rowsShown()[1]!).getByLabelText(/^Quantity/), { target: { value: "20" } });
  fireEvent.click(within(rowsShown()[0]!).getByRole("button", { name: "Remove" }));
  fireEvent.click(screen.getByRole("button", { name: "Add" }));
  fireEvent.change(within(rowsShown()[2]!).getByLabelText(/^Product/), { target: { value: "d" } });
  fireEvent.change(within(rowsShown()[2]!).getByLabelText(/^Quantity/), { target: { value: "4" } });
  fireEvent.click(within(rowsShown()[2]!).getByRole("button", { name: "Move up" }));
  const change = sent() as { create: { ref: string; data: unknown }[]; update: unknown; delete: unknown; order: unknown[] };
  expect(change.update).toEqual([{ key: "2", data: { quantity: 20 } }]);
  expect(change.delete).toEqual(["1"]);
  expect(change.create).toEqual([{ ref: change.create[0]!.ref, data: { product: "d", quantity: 4 } }]);
  // New rows are tagged in the order: a reference never reads as a key.
  expect(change.order).toEqual(["2", { new: change.create[0]!.ref }, "3"]);
});

it("shows the server's problems in their row, by key", async () => {
  mount();
  await screen.findByText("b");
  act(() => form!.setError("lines", { type: "server", message: "Some rows need attention.", types: { "2.quantity": "is required" } }));
  expect(within(rowsShown()[1]!).getByRole("alert")).toHaveTextContent("is required");
  expect(within(rowsShown()[0]!).queryByRole("alert")).toBeNull();
});

it("checks a new row with the child's create inputs, a stored row with its edit inputs", () => {
  const field = (name: string, required: boolean) =>
    ({ name, label: name, type: "text_input", valueType: "string", nullable: !required, disabled: false, rules: required ? [{ rule: "required" }] : [], columnSpan: 1 }) as InputSchema;
  const repeater = {
    components: [field("title", false), field("note", true)],
    createComponents: [field("title", true)],
    addable: true,
    deletable: true,
  } as unknown as RepeaterSchema;
  // A new row has no `note` (edit only): not required there; its `title` is.
  expect(relationRowProblems(repeater, [{ ref: "n1", values: { title: "x" }, changed: [] }])).toEqual({});
  expect(Object.keys(relationRowProblems(repeater, [{ ref: "n1", values: { title: "" }, changed: [] }]))).toEqual(["0.title"]);
  // A stored row (second) is checked with the edit inputs.
  expect(Object.keys(relationRowProblems(repeater, [
    { ref: "n1", values: { title: "x" }, changed: [] },
    { key: "5", values: { title: "", note: "" }, changed: [] },
  ]))).toEqual(["1.note"]);
  // The list's limits apply to every row together, not to each one.
  const two = { ...repeater, minItems: 2 } as RepeaterSchema;
  const rows = [{ ref: "n1", values: { title: "x" }, changed: [] }, { ref: "n2", values: { title: "y" }, changed: [] }];
  expect(relationRowProblems(two, rows)).toEqual({});
  expect(Object.keys(relationRowProblems(two, rows.slice(0, 1)))).toEqual([""]);
});
