# ADR 0037: Inputs and layouts shown under conditions

- Status: accepted (unreleased, vocabulary 13)
- Date: 2026-11-15

## Context

Forms often show an input only for some values of another: a URL when the
link type is `external`, a discount when the product is on sale, answer
options for choice questions only. Today every input of a form is always
shown and always accepted. The only conditional element is
`Action::visible_when`, a Rust closure over the model evaluated on the
server: the SPA cannot run it while the user types.

Constraints from the existing design:

- **The server decides** (field rules, scope, policy on every path). A
  condition hidden by the SPA must not let a value through, and old SPAs
  (which ignore conditions) still send every input.
- **Validation runs before the row is locked.** `accept_input`
  (`engine/adapter/input.rs`) checks the input before
  `find_authorized_locked_moving` reads the stored record; on update the
  values a condition depends on may not be in the input.
- **Required fields** are enforced on create by `accept_input` ("is
  required" for a missing required field): wrong for an input the user never
  saw.
- Forms are shared by resources, settings pages and action forms (the same
  `Input` and `Layout` types).

## Decision

### Declaring

- `shown_when(condition)` on an `Input` or a `Layout` (`Section`, `Grid`,
  `Tabs`, `Tab`, `Split`, `Pane`). The name differs from
  `Action::visible_when` on purpose: that one is a closure the server runs
  on a model, this one is data both sides evaluate.
- Conditions are built with `when(name)`:
  `eq(v)`, `ne(v)`, `is_in([v, …])`, `filled()`, `is_true()`, combined with
  `Condition::all([…])` and `Condition::any([…])`. No closures: the same data is evaluated by the
  SPA and the server.
- `clear_when_hidden()` on an input: when the input is hidden after a write,
  its field is set to the input's **cleared value**: the column's declared
  default (entity `default_value`, the value create forms start with) when
  it has one, else `null` when the column is nullable, else the kind's empty
  value (`""` for text, `[]` for a JSON list).
- A condition names an input of the same form level or of an ancestor level
  (repeater rows, ADR for A3/A4); for now every form has one level.

### Meaning

- An input is **shown** when its own condition and the conditions of every
  enclosing layout hold. A field with several inputs is shown when one of
  them is.
- A condition reads the **resulting values**: on update the stored record
  merged with the input, on create the input merged with the inputs'
  declared defaults (`null` when none), on settings the stored group merged
  with the input, on action forms the submitted data (every field present).
- A **hidden** input's value counts as `null` for other conditions, so chains
  resolve in dependency order: if `b` is hidden, a condition `when("b").eq(x)`
  is false.
- Values are compared in canonical wire form (`i64` as strings without
  leading zeros), decimals by value as written, never rounded to the
  column's scale (`"10.50"` is `"10.5"`, `"10.504"` is not), exactly as the
  SPA compares them.

### Writing (server)

1. `accept_input` keeps checking every value sent (kind, length, options,
   bounds): a malformed value is refused even for a hidden input. It no
   longer reports missing required fields of inputs under a condition.
2. After the row is locked (update) or before the insert (create), in the
   write transaction and before `mutate_before_*` hooks: evaluate the
   conditions on the resulting values; **drop** values sent for hidden
   inputs (old SPAs send them; they are never stored); on **create** a
   hidden input's column takes its database default, or `null`; for hidden
   inputs with
   `clear_when_hidden`, write the cleared value; for **shown** inputs whose
   field is required, report "is required" when the resulting value is
   missing or `null`.
3. Hooks write what they decide; conditions are not evaluated again after
   them.
4. Same evaluation in the settings save (under its version lock) and in
   action admission (before the handler).
5. CSV imports go through the same create and update paths; the import
   preview applies the conditions to rows it would create (an update's
   depend on the stored record, read only when the import runs).
6. `clear_when_hidden` writes only for a user who may write the field.

### Boot checks

A form is refused when a condition names an input that is not in the form
(or an ancestor level); an operator does not fit the column kind (`is_true`
on a boolean only; `eq`, `ne`, `is_in` on scalar kinds, not JSON, rich text,
media lists or key/value; values must decode as the field's kind); the
conditions form a cycle; an encrypted field has a condition (records hold
its value as `null`); a relationship field has `clear_when_hidden` (clearing
it would move the record after its scope and parents were checked and
locked); an input under a condition (its own or a
layout's) has a field whose column is not nullable and has no default
(a create could not store it while hidden: the error would reach the user at
save, on a field they cannot see; declare a default on the column, or make it
nullable); or a `clear_when_hidden` input has no storable cleared value (a
non-nullable number, date or boolean column without a declared default).

### Wire (vocabulary 13)

`InputSchema` and `LayoutSchema` gain `shownWhen` (absent: always shown) and
`InputSchema` gains `clearWhenHidden`. A condition is tagged by `op`:
`{"op": "eq", "name": "link_type", "value": "external"}`, `ne` alike,
`{"op": "is_in", "name": …, "values": [...]}`, `{"op": "filled", "name": …}`,
`{"op": "is_true", "name": …}`, `{"op": "all", "conditions": [...]}` or
`{"op": "any", "conditions": [...]}`. The SPA hides what does not hold, never
sends hidden values, and evaluates the same conditions on the record's
values on view pages. Older SPAs see vocabulary 13 as unknown: they keep
showing every input, and the server ignores what they send for hidden ones.

### Live in the browser

- The SPA evaluates conditions on **every change** of the form's values
  (each keystroke, selection or toggle), locally: no request, no `live()`
  flag on the inputs a condition names (unlike Filament, whose closures run
  on the server and need `->live()` for a round trip). The server evaluates
  them once more when the form is saved, and decides.
- A hidden input is **not rendered** (no space, no focus, no client-side
  validation), but its draft value stays in the form state: shown again
  before saving, it shows what the user typed. Hidden values are never
  sent.
- Conditions only compare values. Logic that needs the server (a query, a
  service) belongs to form reactions (ADR to come), which call the server
  with a debounce.

### Parity

`fixtures/conditions/*.json` lists cases (form, values → shown names); a
Rust test and a Vitest test both run them.

## Consequences

- Hidden values are never stored, whatever the client. A user cannot clear a
  hidden field except through `clear_when_hidden`.
- `required` on a field under a condition means "required when shown".
- A non-nullable column needs a default (entity `default_value` or a
  database default) for its input to be conditional (checked at boot).
  Applications that fill such a field in a hook still declare one; the
  hook's value replaces it. No new input API is needed.
- A condition on a field the user may not read is evaluated by the server on
  the stored value, while the SPA, which does not receive that value, treats
  it as `null`: the two can disagree (the SPA shows an input whose value the
  server then drops). Field rules depend on the user, so boot cannot rule
  this out; the guide tells applications to condition only on fields every
  user of the form may read.
- Tables, lists and exports are unaffected (conditions belong to forms).
- `InputSchema`, `LayoutSchema` gain members: struct literals in
  applications need them.
- Part B (infolist entries, ADR to come) reuses the same conditions on
  stored values; the repeater ADR extends names to ancestor levels and
  `clear_when_hidden` to relation rows.
