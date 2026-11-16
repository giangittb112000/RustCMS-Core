// Form reactions (ADR 0041): what the form sends to the server's reaction,
// and the changes it answers with applied to the form's values. The server
// decides; these only help the user while typing (the save is checked again).
import type { FormPatch, InputSchema, JsonValue } from "@/api/types";
import { isResourceSecret } from "./secrets";
import { type FormValue, type FormValues, fromFormValue, toFormValue } from "./form";
import { parseRows, rowOf } from "./repeater";

/** Pause in typing before a reaction is asked for. */
export const REACTION_DELAY_MS = 400;

/** Inputs whose values a reaction never receives: secrets, links, child records, write-only ones. */
const withheld = (input: InputSchema) =>
  isResourceSecret(input) ||
  input.type === "secret" ||
  input.type === "secret_map" ||
  input.type === "belongs_to_many" ||
  (input.type === "repeater" && !!input.repeater?.relationship) ||
  !!input.writeOnly;

/** The form's values in wire format, as a reaction receives them (a value that does not convert is left out). */
export function reactionState(inputs: InputSchema[], values: FormValues): Record<string, JsonValue> {
  const out: Record<string, JsonValue> = {};
  for (const input of inputs) {
    const value = values[input.name];
    if (value === undefined || withheld(input)) continue;
    try {
      const wire = fromFormValue(input, value);
      if (wire !== undefined) out[input.name] = wire as JsonValue;
    } catch {
      // Half typed (a JSON text, a number): the server would drop it anyway.
    }
  }
  return out;
}

/** Warnings under rows, by repeater and row index. */
export type RowFlags = Record<string, Record<number, string[]>>;

export interface Applied {
  /** New form values, by input. */
  values: Record<string, FormValue>;
  flags: RowFlags;
  notices: string[];
}

const empty = (value: unknown) =>
  value === null || value === undefined || value === "" || (Array.isArray(value) && value.length === 0);

/**
 * `patches` applied in order to `values` (each sees the previous ones' result).
 * A patch naming an input the form does not have, or rows of an input that is
 * not a JSON repeater, is skipped: the server already dropped what the user
 * could not do.
 */
export function applyPatches(inputs: InputSchema[], values: FormValues, patches: FormPatch[]): Applied {
  const byName = new Map(inputs.map((i) => [i.name, i]));
  const current = (name: string): FormValue | undefined => (name in out.values ? out.values[name] : values[name]);
  const out: Applied = { values: {}, flags: {}, notices: [] };
  for (const patch of patches) {
    if (patch.op === "notice") {
      out.notices.push(patch.message);
      continue;
    }
    const input = byName.get(patch.field);
    if (!input || input.disabled || withheld(input)) continue;
    if (patch.op === "set" || patch.op === "set_if_empty") {
      if (patch.op === "set_if_empty") {
        const value = current(input.name);
        let wire: unknown = null;
        try {
          wire = value === undefined ? null : fromFormValue(input, value);
        } catch {
          wire = value;
        }
        if (!empty(wire)) continue;
      }
      out.values[input.name] = toFormValue(input, patch.value);
      continue;
    }
    const repeater = input.repeater;
    if (input.type !== "repeater" || !repeater) continue;
    const rows = parseRows(current(input.name) ?? "");
    if (rows === null) continue;
    switch (patch.op) {
      case "add_row": {
        const at = patch.index ?? rows.length;
        if (at > rows.length) continue;
        const row = patch.row && typeof patch.row === "object" && !Array.isArray(patch.row) ? (patch.row as Record<string, unknown>) : {};
        rows.splice(at, 0, rowOf(repeater, row, true));
        break;
      }
      case "remove_row":
        if (patch.index >= rows.length) continue;
        rows.splice(patch.index, 1);
        break;
      case "move_row":
        if (patch.from >= rows.length || patch.to >= rows.length) continue;
        rows.splice(patch.to, 0, rows.splice(patch.from, 1)[0]!);
        break;
      case "flag_row": {
        if (patch.index >= rows.length) continue;
        const flags = (out.flags[input.name] ??= {});
        (flags[patch.index] ??= []).push(patch.message);
        continue;
      }
    }
    out.values[input.name] = JSON.stringify(rows);
  }
  return out;
}
