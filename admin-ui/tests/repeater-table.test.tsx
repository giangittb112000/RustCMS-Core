// Repeaters in tables and history (ADR 0040, part 3): count columns, a
// repeater's change line by row, a relationship repeater's change by key.
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { AuditEntryView, ColumnSchema, Envelope } from "@/api/types";
import { AuditEntries, rowChanges } from "@/components/audit";
import { followRows } from "@/renderers/repeater";
import { Cell } from "@/renderers/table";

const record = (data: object, counts?: object) => ({ key: "1", abilities: {}, data, counts }) as unknown as Envelope;

it("counts the items of a JSON list, and shows the server's count of a relation", () => {
  const items = { name: "faq", type: "text", label: "FAQ", valueType: "json", count: true } as unknown as ColumnSchema;
  const lines = { name: "lines_count", type: "text", label: "Lines", valueType: "i32", counts: "lines" } as unknown as ColumnSchema;
  const { container, rerender } = render(<Cell column={items} record={record({ faq: [{ q: "a" }, { q: "b" }] })} />);
  expect(container.textContent).toBe("2");
  rerender(<Cell column={items} record={record({ faq: null })} />);
  expect(container.textContent).toBe("0");
  rerender(<Cell column={lines} record={record({}, { lines: 3 })} />);
  expect(container.textContent).toBe("3");
  // No count sent (not listable, too many to check): an empty cell, never a guess.
  rerender(<Cell column={lines} record={record({})} />);
  expect(container.textContent).toBe("");
});

it("lists a repeater's change by row and input", () => {
  const before = [{ q: "a", answer: "x" }, { q: "b", answer: "y" }];
  const after = [{ q: "a", answer: "z" }, { q: "b", answer: "y" }, { q: "c", answer: "w" }];
  expect(rowChanges("faq", before, after)).toEqual([
    ["faq › 1 › answer", "x", "z"],
    ["faq › 3", undefined, { q: "c", answer: "w" }],
  ]);
  expect(rowChanges("faq", [{ q: "a" }], null)).toEqual([["faq › 1", { q: "a" }, undefined]]);
  // Not rows: shown as one value.
  expect(rowChanges("tags", ["a"], ["a", "b"])).toBeNull();
  expect(rowChanges("faq", "[redacted]", [{ q: "a" }])).toBeNull();
  // A list the audit log cut, or no difference told apart: one value, as recorded.
  expect(rowChanges("faq", [{ q: "a" }], [{ q: "b" }, { more: 3 }])).toBeNull();
  expect(rowChanges("faq", [{ q: "a", "…": { more: 2 } }], [{ q: "b" }])).toBeNull();
  expect(rowChanges("faq", [{ n: 1 }], [{ n: 1 }])).toBeNull();
});

it("shows the history of rows in place of raw values", async () => {
  const entry = (over: object) => ({
    id: "1", at: "2026-11-16T08:00:00Z", actor: { kind: "user", id: "1", name: "Admin" }, subjectType: "orders",
    subjectKey: "7", ...over,
  }) as unknown as AuditEntryView;
  render(
    <AuditEntries showSubject={false} entries={[
      entry({ id: "1", event: "updated", changes: { faq: [[{ q: "a" }], [{ q: "b" }]] } }),
      entry({ id: "2", event: "rows_changed", meta: { relation: "lines", created: 1, updated: 0, deleted: 2, reordered: true } }),
    ]} />,
  );
  for (const button of screen.getAllByRole("button")) await userEvent.click(button);
  expect(screen.getByText("faq › 1 › q")).toBeInTheDocument();
  // Counts only: which rows is the child's own history.
  expect(screen.getByText("1")).toBeInTheDocument();
  expect(screen.getByText("2")).toBeInTheDocument();
  expect(screen.getByText("Moved")).toBeInTheDocument();
  expect(screen.queryByText("Changed")).toBeNull();
  expect(screen.queryByText(/"relation"/)).toBeNull();
});

it("keeps the server's problems on the rows they named after rows move", () => {
  const problems = { "": "too many", "0.question": "is required", "2.links.0.href": "is required" };
  // Row a (0) removed, c (2) moved first: its problem follows it; a's is gone.
  expect(followRows(problems, ["a", "b", "c"], ["c", "b"])).toEqual({ "": "too many", "0.links.0.href": "is required" });
});
