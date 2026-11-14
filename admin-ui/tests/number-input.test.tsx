import { fireEvent, render, screen } from "@testing-library/react";
import { useForm } from "react-hook-form";
import type { InputSchema } from "@/api/types";
import { Components } from "@/renderers/form";
import { type FormValues, defaultValues, zodSchema } from "@/schema/form";

const number = (valueType: "i32" | "f64" | "i64", extra: Partial<InputSchema> = {}): InputSchema => ({
  name: "level", label: "Level", type: "number_input", valueType, nullable: true,
  disabled: false, rules: [], columnSpan: 1, ...extra,
});

let values: FormValues = {};
function Harness({ input, data }: { input: InputSchema; data: Record<string, unknown> }) {
  const form = useForm<FormValues>({ defaultValues: defaultValues([input], data) });
  values = form.watch();
  return <Components components={[input]} ctx={{ slug: "gauges", control: form.control, errors: {} }} />;
}

/** Types into a native number input as a browser does when it cannot read the text. */
function typeUnreadable(field: HTMLInputElement, event: "change" | "input" = "change") {
  Object.defineProperty(field, "validity", { configurable: true, value: { badInput: true } });
  fireEvent[event](field, { target: { value: "" } });
}

it("renders i32 and f64 as native number inputs with their bounds, i64 as text", () => {
  const { unmount } = render(<Harness input={number("i32", { number: { min: 1, max: 5 } })} data={{ level: 3 }} />);
  const field = screen.getByLabelText(/^Level/) as HTMLInputElement;
  expect(field).toHaveAttribute("type", "number");
  expect(field).toHaveAttribute("min", "1");
  expect(field).toHaveAttribute("max", "5");
  expect(field).toHaveAttribute("step", "1");
  expect(field).toHaveAttribute("inputmode", "numeric");
  unmount();
  // Negatives allowed: no digits-only keypad (it has no minus).
  const signed = render(<Harness input={number("i32", { number: { min: -10 } })} data={{ level: 3 }} />);
  expect(screen.getByLabelText(/^Level/)).not.toHaveAttribute("inputmode");
  signed.unmount();
  render(<Harness input={number("i64", { number: { max: 10 } })} data={{ level: "3" }} />);
  expect(screen.getByLabelText(/^Level/)).not.toHaveAttribute("type", "number");
});

it("reports text a native number input cannot read instead of clearing the field", () => {
  const input = number("f64");
  render(<Harness input={input} data={{ level: 15 }} />);
  const field = screen.getByLabelText(/^Level/) as HTMLInputElement;
  typeUnreadable(field);
  // The field is not emptied (which would save null): the form refuses it.
  const parsed = zodSchema([input]).safeParse(values);
  expect(parsed.success).toBe(false);
  expect(parsed.error?.issues[0]?.message).toBe("must be a number");

  // Clearing unreadable text leaves the value "" (no change event): the field is empty again.
  Object.defineProperty(field, "validity", { configurable: true, value: { badInput: false } });
  fireEvent.input(field, { target: { value: "" } });
  expect(values.level).toBe("");
  expect(zodSchema([input]).safeParse(values).success).toBe(true);

  // Typing a lone `-` into the empty field: still "" for the input, reported.
  typeUnreadable(field, "input");
  expect(zodSchema([input]).safeParse(values).success).toBe(false);

  // Text it can read is a value again.
  Object.defineProperty(field, "validity", { configurable: true, value: { badInput: false } });
  fireEvent.change(field, { target: { value: "2.5" } });
  expect(zodSchema([input]).safeParse(values).success).toBe(true);
});
