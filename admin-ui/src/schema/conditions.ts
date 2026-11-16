// Conditions under which inputs and layouts are shown (ADR 0037), evaluated
// in the browser on every change. The server evaluates the same conditions on
// every write and decides (`crates/rustcms-admin/src/engine/conditions.rs`);
// both run the cases of `fixtures/conditions`.
import type { ComponentSchema, Condition, JsonValue } from "@/api/types";

/** Values in wire form, by input name. */
export type WireValues = Record<string, unknown>;

/** The conditions of a form, with what is needed to evaluate them. */
export interface FormConditions {
  /** Fields whose every input is under a condition, dependencies first: shown when one condition holds. */
  fields: { name: string; conditions: Condition[] }[];
  /** Wire kind (`valueType`) of each input. */
  kinds: Map<string, string>;
}

const isInputSchema = (c: ComponentSchema): c is Extract<ComponentSchema, { name: string }> => "name" in c;

const both = (a: Condition | undefined, b: Condition | undefined): Condition | undefined =>
  a && b ? { op: "all", conditions: [a, b] } : (a ?? b);

/** The conditions of `components` (inputs inherit the conditions of their layouts). */
export function formConditions(components: ComponentSchema[]): FormConditions {
  const kinds = new Map<string, string>();
  const byField = new Map<string, { conditions: Condition[]; always: boolean }>();
  const walk = (list: ComponentSchema[], inherited: Condition | undefined) => {
    for (const c of list) {
      if (isInputSchema(c)) {
        kinds.set(c.name, c.valueType);
        const entry = byField.get(c.name) ?? { conditions: [], always: false };
        const condition = both(inherited, c.shownWhen);
        if (condition) entry.conditions.push(condition);
        else entry.always = true;
        byField.set(c.name, entry);
      } else {
        walk(c.components, both(inherited, c.shownWhen));
      }
    }
  };
  walk(components, undefined);
  const conditional = [...byField].filter(([, e]) => !e.always).map(([name, e]) => ({ name, conditions: e.conditions }));
  // Dependencies first (the server refuses cycles at boot; a cycle here is just cut).
  const index = new Map(conditional.map((f, i) => [f.name, i]));
  const sorted: number[] = [];
  const state = new Array<number>(conditional.length).fill(0);
  const visit = (i: number) => {
    if (state[i] !== 0) return;
    state[i] = 1;
    for (const c of conditional[i]!.conditions) {
      for (const name of namesOf(c)) {
        const d = index.get(name);
        if (d !== undefined) visit(d);
      }
    }
    state[i] = 2;
    sorted.push(i);
  };
  conditional.forEach((_, i) => visit(i));
  return { fields: sorted.map((i) => conditional[i]!), kinds };
}

function namesOf(condition: Condition): string[] {
  return condition.op === "all" || condition.op === "any" ? condition.conditions.flatMap(namesOf) : [condition.name];
}

/** The input names the conditions of `components` read, once each (the inputs to watch). */
export function conditionNames(components: ComponentSchema[]): string[] {
  const names = new Set<string>();
  const walk = (list: ComponentSchema[]) => {
    for (const c of list) {
      if (c.shownWhen) namesOf(c.shownWhen).forEach((n) => names.add(n));
      if (!isInputSchema(c)) walk(c.components);
    }
  };
  walk(components);
  return [...names];
}

/** Whether the form has any condition (a form without needs no watching). */
export const hasConditions = (components: ComponentSchema[]): boolean =>
  components.some((c) => c.shownWhen !== undefined || (!isInputSchema(c) && hasConditions(c.components)));

/** The fields hidden for `values` (missing values are null; a hidden field's value is null for the conditions after it). */
export function hiddenFields(form: FormConditions, values: WireValues): Set<string> {
  const hidden = new Set<string>();
  for (const field of form.fields) {
    if (!field.conditions.some((c) => holds(form, c, values, hidden))) hidden.add(field.name);
  }
  return hidden;
}

/** Whether `condition` holds for `values`, with `hidden` fields read as null. */
export function holds(form: FormConditions, condition: Condition, values: WireValues, hidden: Set<string>): boolean {
  if (condition.op === "all") return condition.conditions.every((c) => holds(form, c, values, hidden));
  if (condition.op === "any") return condition.conditions.some((c) => holds(form, c, values, hidden));
  const value = hidden.has(condition.name) ? null : (values[condition.name] ?? null);
  const kind = form.kinds.get(condition.name) ?? "string";
  switch (condition.op) {
    case "eq":
      return same(kind, value, condition.value);
    case "ne":
      return !same(kind, value, condition.value);
    case "is_in":
      return condition.values.some((v) => same(kind, value, v));
    case "filled":
      return filled(value);
    case "is_true":
      return value === true;
  }
}

/** Set: not null, blank text, `[]` or `{}`. */
export function filled(value: unknown): boolean {
  if (value === null || value === undefined) return false;
  if (typeof value === "string") return value.trim() !== "";
  if (Array.isArray(value)) return value.length > 0;
  if (typeof value === "object") return Object.keys(value).length > 0;
  return true;
}

function same(kind: string, a: unknown, b: JsonValue | unknown): boolean {
  const x = canonical(kind, a);
  const y = canonical(kind, b);
  return x === y || (x !== null && typeof x === "object" && JSON.stringify(x) === JSON.stringify(y));
}

/**
 * A value as compared: integers and decimals numerically (`"007"` is `"7"`,
 * `"10.50"` is `"10.5"`), UUIDs in lower case; values that do not read as
 * their kind compare as written.
 */
function canonical(kind: string, value: unknown): unknown {
  if (value === null || value === undefined) return null;
  switch (kind) {
    case "i64":
      try {
        return typeof value === "string" || typeof value === "number" ? BigInt(value).toString() : value;
      } catch {
        return value;
      }
    case "decimal":
      return typeof value === "string" && /^-?\d+(\.\d+)?$/.test(value) ? decimal(value) : value;
    case "i32":
    case "f64":
      return typeof value === "string" && value.trim() !== "" && Number.isFinite(Number(value)) ? Number(value) : value;
    case "uuid":
      return typeof value === "string" ? canonicalUuid(value) : value;
    default:
      return value;
  }
}

function decimal(text: string): string {
  const negative = text.startsWith("-");
  const [whole = "0", fraction = ""] = (negative ? text.slice(1) : text).split(".");
  const w = whole.replace(/^0+(?=\d)/, "");
  const f = fraction.replace(/0+$/, "");
  const out = f ? `${w}.${f}` : w;
  return negative && out !== "0" ? `-${out}` : out;
}

/**
 * A UUID as the server writes it (lower case, hyphenated): it also reads 32
 * hex digits without hyphens, in braces, or as a `urn:uuid:`. Anything else
 * stays as written, lower case.
 */
export function canonicalUuid(text: string): string {
  const lower = text.toLowerCase();
  const bare = lower.replace(/^urn:uuid:/, "").replace(/^\{(.*)\}$/, "$1").replace(/-/g, "");
  if (!/^[0-9a-f]{32}$/.test(bare)) return lower;
  return `${bare.slice(0, 8)}-${bare.slice(8, 12)}-${bare.slice(12, 16)}-${bare.slice(16, 20)}-${bare.slice(20)}`;
}
