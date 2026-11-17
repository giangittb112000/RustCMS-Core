// Relationship repeaters (ADR 0040): the rows are child records. The form
// holds what the user changed (empty: nothing); a save sends a change
// (create, update, delete, order), never the whole list, so rows another user
// added meanwhile, and rows the user cannot see, are never touched.
import type { Envelope, InputSchema, JsonValue, RepeaterSchema } from "@/api/types";
import { type FormValue, fromFormValue, inputsOf } from "./form";
import { type Row, rowOf } from "./repeater";

/** A row: a stored child (`key`) or a new one (`ref`), its form values and what changed. */
export interface RelationRow {
  key?: string;
  ref?: string;
  values: Row;
  /** Inputs the user changed (stored rows send only these). */
  changed: string[];
  /** Whether the user may change or remove this stored row. */
  update?: boolean;
  delete?: boolean;
}

export interface RelationState {
  rows: RelationRow[];
  /** Stored rows the user removed. */
  deleted: string[];
  /** The user moved rows: the order is sent (else a row added meanwhile would make it incomplete). */
  reordered?: boolean;
}

/** The rows of the children as read from the relation's list. */
export function stateOf(repeater: RepeaterSchema, records: Envelope[]): RelationState {
  // In the order column's order when there is one (the list reads in key order).
  const column = repeater.relationship?.orderColumn;
  const position = (r: Envelope) => (column ? Number(r.data[column] ?? Infinity) : 0);
  const ordered = column ? [...records].sort((a, b) => position(a) - position(b)) : records;
  return {
    rows: ordered.map((r) => ({
      key: r.key,
      values: rowOf(repeater, r.data, true),
      changed: [],
      update: r.abilities.update === true,
      delete: r.abilities.delete === true,
    })),
    deleted: [],
  };
}

/** The state a form value holds, or null when nothing changed yet. */
export function parseState(value: FormValue): RelationState | null {
  if (typeof value !== "string" || value === "") return null;
  try {
    const state = JSON.parse(value) as RelationState;
    return Array.isArray(state.rows) && Array.isArray(state.deleted) ? state : null;
  } catch {
    return null;
  }
}

let nextRef = 0;
/** A reference for a new row, unique in this page (`n1`, `n2`, …). */
export const newRef = () => `n${++nextRef}`;

/** A new record's form value: its declared empty rows, or nothing. */
export function relationFormValue(input: InputSchema): FormValue {
  const repeater = input.repeater;
  const count = repeater?.defaultItems ?? 0;
  if (!repeater || count === 0) return "";
  const rows = Array.from({ length: count }, () => ({ ref: newRef(), values: rowOf(repeater, {}), changed: [] }));
  return JSON.stringify({ rows, deleted: [] } satisfies RelationState);
}

/** The change a form value sends (undefined: nothing changed). */
export function changeOf(input: InputSchema, value: FormValue): JsonValue | undefined {
  const repeater = input.repeater;
  const state = parseState(value);
  if (!repeater || !state) return undefined;
  const inputs = inputsOf(repeater.components);
  const wire = (row: RelationRow, only?: string[]) => {
    const out: Record<string, JsonValue> = {};
    for (const i of inputs) {
      if (only && !only.includes(i.name)) continue;
      const form = row.values[i.name];
      if (form === undefined) continue;
      const v = fromFormValue(i, form) as JsonValue | undefined;
      // A new row leaves its empty inputs to the server's defaults; a change sends what the user cleared.
      if (v === undefined || (!only && (v === null || v === ""))) continue;
      out[i.name] = v;
    }
    return out;
  };
  const change: Record<string, JsonValue> = {};
  // One reference per new row, used by `create` and `order` alike.
  const refs = new Map(state.rows.filter((r) => !r.key).map((r) => [r, r.ref ?? newRef()] as const));
  const create = state.rows.filter((r) => !r.key).map((r) => ({ ref: refs.get(r)!, data: wire(r) }));
  const update = state.rows.filter((r) => r.key && r.changed.length > 0).map((r) => ({ key: r.key!, data: wire(r, r.changed) }));
  if (create.length) change.create = create;
  if (update.length) change.update = update;
  if (state.deleted.length) change.delete = state.deleted;
  // Stored rows by key, new rows tagged (`{"new": ref}`): a ref never reads as a key.
  if (repeater.reorderable && state.reordered) change.order = state.rows.map((r) => (r.key ? r.key : { new: refs.get(r)! }));
  return change;
}
