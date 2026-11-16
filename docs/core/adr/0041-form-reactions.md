# ADR 0041: Form reactions

- Status: accepted (unreleased, vocabulary 16)
- Date: 2026-11-16

## Context

A form cannot react to what the user types. Common cases:

- a blog or a company site fills a slug from the title, unless the user typed one;
- a shop totals the lines of an order as they are edited;
- a course platform keeps the blanks of a question in step with its text (an
  application rule: the framework only runs it).

Today an application can compute these in `mutate_before_create`/`update`,
but the user only sees the result after saving, and cannot correct it before.

Constraints from the existing design:

- **The SPA draws from data** (ADR 0002): Rust closures cannot run in the
  browser. A reaction runs on the server and answers with changes to apply.
- **The server decides.** Field rules, `scope()`, policies and conditions
  (ADR 0037) hold on every path; what the browser applied is never trusted.
- **`computed()` (ADR 0039) already runs application code on demand** for one
  page: an async closure with `Ctx`, a time limit, a concurrency bound, a rate
  limit per user, failures logged and shown as "not available". Reactions
  follow it.
- Forms of settings pages (`SettingField`) and actions (`ActionField`) are
  their own types, not `Input`.
- JSON repeater rows have no stable ids yet (`.row_ids` is left for later);
  relationship repeater rows have keys or client refs (ADR 0040).

## Decision

### Declaring

```rust
fn form() -> Form {
    Form::new().schema(components![
        TextInput::make("name").reacts(|_ctx, state| {
            let slug = slugify(state.text("name"));
            Box::pin(async move { Ok(vec![FormPatch::set_if_empty("slug", slug)]) })
        }),
        TextInput::make("slug"),
        Repeater::make("lines").schema(components![/* … */]).reacts(order_service::totals),
        RichEditor::make("content").reacts(question_service::sync_blanks).strict(),
    ])
}
```

- `Input::reacts(f)` with
  `f: for<'a> Fn(&'a Ctx, &'a FormState) -> BoxFuture<'a, AppResult<Vec<FormPatch>>>`:
  async, with the request's `Ctx`, so it may read the database (as
  `Computed::make`).
- `.strict()`: a save is refused while the reaction still has changes to make
  (see *Strict reactions*).
- `.reaction_timeout(d)`: default 5 seconds, at most 30.
- Resources only. Settings pages and action forms are left for later.

`FormState` is what the user has in the form:

- `operation()` (`Create` or `Edit`), `key()` (the record's, on edit), `changed()`
  (the input whose change asked for the reaction; on save, its own name);
- `get(name) -> Option<&Value>` (wire format, ADR 0003), `text(name) -> &str`
  (`""` when absent or not a string), `rows(name) -> &[Value]` (a JSON
  repeater's rows; empty when absent).

It holds **only fields the user may read**, never encrypted fields, links
(`BelongsToManySelect`) or relationship repeaters.

### Patches

| Patch | Effect |
|---|---|
| `set(field, value)` | sets the field |
| `set_if_empty(field, value)` | sets it when it is `null`, `""` or `[]` |
| `add_row(field, row)` / `add_row_at(field, index, row)` | adds a row to a JSON repeater (at the end, or at `index`) |
| `remove_row(field, index)` | removes a row |
| `move_row(field, from, to)` | moves a row |
| `flag_row(field, index, message)` | a warning under a row; changes nothing |
| `notice(message)` | a message to the user; changes nothing |

- Values are in wire format. Patches apply in order: a row index refers to
  the rows as the previous patches left them.
- Messages are translated with `ctx.t()`, as hook messages are.
- Row patches apply to **JSON repeaters only**; relationship repeaters are
  left for later (they need patches by row key).

The server keeps a patch only if:

- its field is an input of the form, at the top level, that the user may
  **read and write** in this operation (the value lands in the user's form);
- the field is not hidden by its conditions (ADR 0037) on the state;
- the field is not encrypted, a link list or a relationship repeater;
- the value passes the input's checks (`check_input`: kind, options, bounds,
  length; `required` is not checked) and, for rows, the repeater's row checks
  on the resulting list;
- row indexes are in range;
- it changes something (`set` to the current value and `set_if_empty` on a
  filled field are dropped).

A dropped patch is logged (`tracing::warn!`, resource, field and reason,
never values): it is the application's bug, not the user's.

### The route

`POST {admin}/api/resources/{slug}/form/react`, listed in `http::routes` and
in `fixtures/authz/admin.toml`.

```json
{ "operation": "create", "key": null, "changed": "name", "state": { "name": "Hello" }, "seq": 7 }
→ { "seq": 7, "patches": [{ "op": "set_if_empty", "field": "slug", "value": "hello" }] }
```

- `operation`: `create` needs `CREATE` (policy, no record); `edit` needs
  `key`, `UPDATE` admitted, the record found within `scope()` and the policy
  allowing `UPDATE` on it (404/403 as `update`). `view`, or `edit` without a
  key, is 422. **Reactions never run for view pages, tables or exports.**
- `changed` must name an input with `.reacts()` the user may write; otherwise 422.
- `state`: fields the user may not read and unknown names are dropped; a
  value that does not decode is dropped (the form may be half typed, so no
  422). On edit, readable fields not sent take their stored values.
- Only the reaction of `changed` runs. Patches the SPA applies do not trigger
  further reactions (no loops).
- Errors and timeouts answer `{seq, patches: [], failed: true}` and are logged.
- Limits: 120 requests per user per minute (`form-react:{user}`, the
  `computed` limiter); a bounded number of reactions at once (a semaphore,
  as `COMPUTING`); the usual body limit.
- Nothing is stored, nothing is audited.

### Strict reactions

On every write path (the form, the admin API, CSV import): in `create` and
`update`, after `apply_conditions` and before `mutate_before_*`, each
`.strict()` reaction runs on the resulting values (the locked record merged
with the input; on create, the columns' defaults merged with the input),
with the same `FormState` rules (readable fields only) and `changed` set to
its own input.

- If a patch remains after filtering (a change, or `flag_row`), the write is
  refused: 422 on the reaction's field, "is not up to date". `notice` does
  not count.
- An error or a timeout refuses the write: 422 on the field, "could not be
  checked", logged.
- A reaction whose input is hidden by its conditions does not run.
- Patches on fields the user may not write are dropped as on the route, so a
  strict reaction only refuses what the user could have fixed.
- An import's preview does not run reactions (it checks rows without the
  database); a row out of step is refused when the import runs, in its report.

### Schema (vocabulary 16)

- `InputSchema.reacts: true` on inputs with a reaction, for `create` and
  `edit` only.
- Wire types `FormReactRequest`, `FormReactResponse`, `FormPatch` (tagged by `op`).
- New validation messages in `rustcms_core::i18n::TEMPLATES`.

### The SPA

- After an input with `reacts` changes, and the user stops typing for about
  400 ms, the SPA sends the form's values in wire format (as a save sends
  them, without secrets, links or relationship rows) with an increasing `seq`.
- A response is applied only if its `seq` is the latest **and** the form has
  not changed since it was sent (a newer request is then on its way).
- `set`/`set_if_empty` and row patches go through `setValue` (dirty, not
  validated); `flag_row` shows a warning under the row until the next
  response; `notice` shows a toast.
- Network errors, 429 and `failed` are ignored silently: the form still works
  without reactions, and the save is checked anyway.
- An SPA of vocabulary 15 ignores `reacts`; a strict reaction still holds when
  it saves.

### Boot checks (panic)

- `.reacts()` on an input that is not a field of the resource, on `hidden`,
  encrypted, link or relationship repeater inputs, or inside repeater rows.
- `.strict()` or `.reaction_timeout()` without `.reacts()`; a timeout of zero
  or over 30 seconds.

## Consequences

- Slugs, totals and synchronized rows are declared next to the input, in
  Rust, with access to services and the database; the user sees the result
  before saving.
- The browser calls the server while the user types: one request per pause,
  bounded per user. Reactions should be light; a slow one only fails alone.
- A strict reaction runs on every save, inside the update's transaction
  (after the record is locked): keep it light. Records saved before it was
  declared must be brought in step at their next save, including by imports.
- `InputSchema` gains `reacts`; applications matching it exhaustively need it.
- Left for later: reactions on settings pages and action forms, row patches
  for relationship repeaters (row keys), reactions inside rows, and an SDK
  `registerFormReaction` for immediate feedback in the browser (`ui` 4).
