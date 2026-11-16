// JSON repeaters (ADR 0040), from the server's contract
// (fixtures/contract/repeater.schema.create.json): rows added, filled,
// reordered, copied and removed; conditions inside a row; wire values sent;
// problems shown in their row, the panel's and the server's.
import { act, fireEvent, render, screen, within } from "@testing-library/react";
import { useForm } from "react-hook-form";
import type { ComponentSchema, InputSchema } from "@/api/types";
import { Components, ReadOnlyComponents } from "@/renderers/form";
import { type FormValues, defaultValues, inputsOf, payloadOf, serverErrors } from "@/schema/form";
import schema from "../../fixtures/contract/repeater.schema.create.json";

const components = schema.form.components as unknown as ComponentSchema[];
const faq = components.find((c) => "name" in c && c.name === "faq") as InputSchema;
const inputs = inputsOf(components);

let form: ReturnType<typeof useForm<FormValues>> | undefined;
function Harness({ data, submitCount = 0 }: { data?: Record<string, unknown>; submitCount?: number }) {
  form = useForm<FormValues>({ defaultValues: defaultValues(inputs, data) });
  return <Components components={components} ctx={{ slug: "products", control: form.control, errors: form.formState.errors, submitCount }} />;
}

const rows = () => screen.getAllByRole("listitem");
const sent = () => payloadOf(inputs, form!.getValues()).faq;

it("adds, fills, reorders, copies and removes rows, and sends wire values", () => {
  render(<Harness />);
  expect(screen.getByText("No items yet.")).toBeInTheDocument();
  fireEvent.click(screen.getByRole("button", { name: /Add a question/ }));
  fireEvent.click(screen.getByRole("button", { name: /Add a question/ }));
  expect(rows()).toHaveLength(2);

  fireEvent.change(within(rows()[0]!).getByLabelText(/^Question/), { target: { value: "Size?" } });
  fireEvent.change(within(rows()[0]!).getByLabelText(/^Votes/), { target: { value: "3" } });
  fireEvent.change(within(rows()[1]!).getByLabelText(/^Question/), { target: { value: "Colour?" } });
  // The item label follows the row's question.
  expect(within(rows()[0]!).getByRole("button", { name: /Size\?/ })).toHaveAttribute("aria-expanded", "true");

  fireEvent.click(within(rows()[0]!).getByRole("button", { name: "Move down" }));
  // Empty inputs are left out; an empty nested list is a list.
  expect(sent()).toEqual([{ question: "Colour?", pinned: false, links: [] }, { question: "Size?", votes: 3, pinned: false, links: [] }]);

  fireEvent.click(within(rows()[1]!).getByRole("button", { name: "Duplicate" }));
  expect(rows()).toHaveLength(3);
  // Three is the most: no more adding or copying.
  expect(screen.queryByRole("button", { name: /Add a question/ })).toBeNull();
  expect(within(rows()[0]!).queryByRole("button", { name: "Duplicate" })).toBeNull();

  fireEvent.click(within(rows()[0]!).getByRole("button", { name: "Remove" }));
  const size = { question: "Size?", votes: 3, pinned: false, links: [] };
  expect(sent()).toEqual([size, size]);
});

it("shows a row's inputs under their conditions, and folds rows", () => {
  render(<Harness data={{ faq: [{ question: "Docs?", kind: "text" }] }} />);
  const row = rows()[0]!;
  expect(within(row).queryByLabelText(/^Url/)).toBeNull();
  fireEvent.change(within(row).getByLabelText(/^Kind/), { target: { value: JSON.stringify("link") } });
  expect(within(row).getByLabelText(/^Url/)).toBeInTheDocument();

  const header = within(row).getByRole("button", { name: /Docs\?/ });
  fireEvent.click(header);
  expect(header).toHaveAttribute("aria-expanded", "false");
  expect(within(row).getByLabelText(/^Question/, { selector: "input" }).closest("[hidden]")).not.toBeNull();
});

it("shows the panel's problems in their row once the user tried to save", () => {
  render(<Harness data={{ faq: [{ answer: "No question" }] }} submitCount={1} />);
  expect(within(rows()[0]!).getByRole("alert")).toHaveTextContent("is required");
});

it("routes the server's row problems to the repeater", () => {
  const { byInput, other } = serverErrors(
    { "faq.0.question": ["is required"], "faq.1.links.0.href": ["is required"], name: ["is required"], unknown: ["x"] },
    new Set(["faq", "name"]),
  );
  expect(byInput.faq).toEqual({ message: "Some rows need attention.", types: { "0.question": "is required", "1.links.0.href": "is required" } });
  expect(byInput.name).toEqual({ message: "is required", types: {} });
  expect(other).toEqual([["unknown", ["x"]]]);

  render(<Harness data={{ faq: [{ question: "" }] }} />);
  act(() => form!.setError("faq", { type: "server", ...byInput.faq! }));
  expect(within(rows()[0]!).getByRole("alert")).toHaveTextContent("is required");
});

it("draws rows read-only on view pages", () => {
  render(<ReadOnlyComponents components={[faq]} data={{ faq: [{ question: "Size?", pinned: true, links: [{ href: "/a" }] }] }} slug="products" record="1" />);
  expect(screen.getByText("Size?")).toBeInTheDocument();
  expect(screen.getByText("/a")).toBeInTheDocument();
  expect(screen.getByText("Yes")).toBeInTheDocument();
});
