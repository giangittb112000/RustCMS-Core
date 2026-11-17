# ADR 0039: Infolists for record view pages

- Status: accepted (unreleased, vocabulary 14)
- Date: 2026-11-15

## Context

A record's view page is its form drawn read-only: `GET schema?operation=view`
returns the form's inputs the user may read, and `ReadOnlyComponents`
(`admin-ui/src/renderers/form.tsx`) draws each input kind as text, a date, a
thumbnail, a document. An application cannot show a value differently on
the view page than in the form (a status as a colored badge, a code with a
copy button), add a value that is not a column (the number of learners of a
course), or draw a value with its own component.

Constraints from the existing design:

- **The SPA draws from data.** The server sends a schema (JSON) and the
  records; Rust closures cannot reach the browser. Filament can run
  `TextEntry::make('x')->state(fn ($record) => …)` because the server renders
  the page's HTML; here a computed value must be computed on the server and
  sent as a value.
- **One envelope for every read.** `GET records/{key}` returns the same
  `Envelope` the list, the edit page, the SDK's `useRecord` and every write
  result use. Whatever is added to it is paid for everywhere.
- **Field rules decide what is sent.** A value the user may not read never
  leaves the server; the schema leaves out inputs for such fields.
- The view page holds more than the form: actions, slots, relation managers,
  the record history. None of it is about how fields are displayed.
- Forms are shared by resources, settings pages and action forms (the same
  `Component`, `Input` and `Layout` types).

## Decision

### Declaring

```rust
fn infolist() -> Infolist {
    Infolist::new().columns(2).schema(components![
        Section::make("Order").schema(components![
            TextEntry::make("number").copyable(),
            TextEntry::make("status").badge().color("paid", "success").color("refunded", "danger"),
            RelationEntry::make("customer_id"),
        ]),
        ComputedEntry::make("total"),
        CustomEntry::make("route", "acme/map"),
    ])
}

fn computed() -> Vec<Computed<Self::Entity>> {
    vec![Computed::make("total", |ctx, order: &order::Model| Box::pin(async move {
        Ok(json!(order_service::total(&ctx.db, order.id).await?))
    }))]
}
```

- `Resource::infolist()` returns an empty `Infolist` by default. **Empty: the
  view page stays the read-only form**, as today (Filament's view page
  without an infolist shows its form disabled). **Declared: it replaces the
  form on the view page entirely**; there is no "the form plus a few
  entries" mode. Nothing around the body changes: header, actions, slots,
  relation managers, history.
- Layouts are the form's: `Section`, `Grid`, `Tabs`/`Tab`, `Split`/`Pane`,
  with the same rules (`check_layouts`) and options (`columns`,
  `collapsible`, `compact`, `shown_when`). `Component` gains an `Entry`
  variant, built with the same `components![]` macro. Forms (resource,
  settings, action) may not hold entries, infolists may not hold inputs:
  both are refused at boot.
- `Infolist::width` sets the page width; unset, the form's width applies.

### Entries

An entry names a field of the resource, except `ComputedEntry` (a computed
value of the resource). Common options: `label`, `help`, `column_span`,
`shown_when`.

| Entry | For | Options |
|---|---|---|
| `TextEntry` | any scalar field, JSON lists | `badge()` + `color(value, color)`, `copyable()`, `limit(chars)`, `url()` (the value is the link: `http`/`https` only), `number(decimals)`, `list()` (one item per line), `multiline()`, `password()` (masked), `prefix()`/`suffix()`, `options([(value, label)])` |
| `RelationEntry` | `belongs_to` fields | none: the related record's title, linked (below) |
| `BooleanEntry` | boolean fields | `labels(yes, no)`, `color(true, color)` |
| `KeyValueEntry` | key/value fields | none |
| `MediaEntry` | media fields | `size(px)`, `gallery()` |
| `RichTextEntry` | rich text fields | `outline()` |
| `ComputedEntry` | a computed value | `TextEntry`'s display options |
| `CustomEntry` | any field or computed value | `props(json)` (≤ 4 KiB): drawn by an extension script |

Dates and times are drawn from the field's wire kind, as on today's view
page (instants in the user's time zone). Badge colors are the table's
(`primary`, `success`, `warning`, `danger`, `gray`).

Without options each entry draws exactly what today's read-only form draws
for the matching input (the table of the branch plan); the SPA shares those
drawing functions between both.

### Reading

- An entry whose field the user may not read is left out of the schema; a
  layout left empty is left out. Encrypted fields have no entry (refused at
  boot).
- Entries read the record from the envelope (`data`, `labels`): no new read
  path, no request per entry.
- **`RelationEntry` links** to the related record's view page when the user
  may list the related resource (`view_any`, decided when building the
  schema, sent as `link`) and the envelope's label is the title rather than
  `#key` (the envelope already gives `#key` for a related record the user
  may not view). The related page checks again, as every page does.
- **Conditions** (ADR 0037) are evaluated by the SPA on the stored values.
  They name other entries of the infolist, as a form's conditions name its
  inputs (not computed values, whose kind is unknown); an entry left out for
  the user counts as `null`. On a view page a condition is presentation, not
  security: every readable value is in the envelope anyway.

### Computed values

- `Resource::computed() -> Vec<Computed<Self::Entity>>`.
  `Computed::make(name, |ctx, model| …)` returns
  `AppResult<serde_json::Value>`; the closure gets the typed model, as
  action handlers do. `.readable(|ctx| …)` restricts who may see it (default:
  whoever may view the record), like `Field::readable`. `.timeout(duration)`
  sets its time limit: **5 seconds by default, at most 30** (more is refused
  at boot).
- They are sent by their own route, `GET
  {admin}/api/resources/{slug}/records/{key}/computed`, which the view page
  calls alongside the schema and the record, only when its infolist has a
  `ComputedEntry`. The envelope does not change; the edit page, lists,
  exports and writes never compute.
- The route admits `view`, finds the record in `scope()`, checks
  `policy.allows(view, record)`, then runs the readable computed values the
  infolist names, concurrently, outside any transaction. Never cached,
  stored or written to the audit log.
- **A failure stays in its entry.** An error, a timeout or a value larger
  than 64 KiB puts the name in `failed`; the others are returned. The entry
  shows "not available"; the rest of the page is unaffected. The server logs
  a warning with the resource, the name and the kind of failure, never the
  value. A computed value that times out is cancelled (its future dropped).
- Response: `{"data": {"total": "120.50"}, "failed": ["revenue"]}`. The
  schema marks entries whose value comes from this route (`computed: true`),
  so clients never guess.
- The route runs application code: 60 requests per user per minute (the
  auth limiter), and at most 8 view pages computing at once, a turn awaited
  at most 10 seconds after authorization; beyond, 429.

### Custom entries

`CustomEntry::make(name, "vendor/name")` draws the value with the component
an extension script registered with `registerEntry("vendor/name", …)` (v2
rules, ADR 0032), given `value`, `record` (readable fields), `props`,
`locale` and `t`. A missing or failing script falls back to the value as
text, like a custom cell.

### Outside record pages

- `PageView::Infolist(|| Infolist::new()…)` (a `fn() -> Infolist`, so
  `PageView` stays `Copy`): a read-only panel page (or slot) drawn without a
  script. Entries name keys of `PanelPage::data`, not fields; the page's
  permissions are the only check. Only `TextEntry`, `BooleanEntry`,
  `KeyValueEntry` and `CustomEntry` are allowed there (relations, media,
  rich text and computed values need a record or checks a page does not
  have), and conditions with `filled` only (the values have no declared
  kind): others are refused at boot.
- The SDK's UI kit (level `ui` 3) exports `Infolist` (`schema`, `data`), so a
  React page can look like a record page; `registerEntry` joins
  `registerCell`.

### Boot checks

An infolist is refused when an entry names no field (or, for
`ComputedEntry`, no computed value); its kind does not fit the field
(`RelationEntry` on a non-`belongs_to` field, `MediaEntry` on a non-media
field, `RichTextEntry` on a non-rich-text field, `KeyValueEntry` on a
non-JSON column, `BooleanEntry` on a non-boolean); an option does not fit the
entry or the field (`number` on text, `badge` on a boolean); the field is
encrypted; two entries share a name; a custom entry's name is not
`vendor/name` or its props exceed 4 KiB; a condition names an unknown field
or a computed value, or breaks ADR 0037's rules; layouts break
`check_layouts`; the infolist holds an input, or a form holds an entry. A
computed value is refused when its name is a field's, is declared twice, or
its timeout exceeds 30 seconds.

### Wire (vocabulary 14)

- `ResourceSchema` gains `infolist` (`InfolistSchema`: `columns`,
  `components`, `width`), only for `operation=view` of a resource that
  declares one; absent otherwise, so every other schema is unchanged.
- `ComponentSchema` gains entries: `{"type": "text_entry", "name", "label",
  "valueType", "columnSpan", "shownWhen"?, …options}`; `RelationEntry` adds
  `relationship` and `link`.
- `CustomContent` gains `{"kind": "infolist", "infolist", "data"}`.
- Older SPAs ignore `infolist` and keep drawing the form read-only, which
  shows the same readable values; `VOCABULARY_VERSION` 14 tells them they are
  outdated.

## Consequences

- Applications that declare nothing see no change. A declared infolist
  owns the whole body of the view page: a field missing from it is not
  shown there (it still appears in the form and the list).
- `Component` gains a variant: applications matching on it need a new arm.
- A computed value costs one request per view page, concurrent with the
  others; a slow one delays only its entry. Applications keep them light
  (no query per related row); the guide says so.
- Computed values are readable by whoever may view the record unless
  `.readable` says otherwise: an aggregate can disclose more than the
  fields it is built from, so the guide asks to restrict it like a field.
- The new route needs a case in `fixtures/authz/admin.toml`.
- Making infolists the default view page, and entries for repeaters, came
  later (ADR 0040, and the amendment below).

## Amendment (0.13.1): the default view page

A resource without `infolist()` now gets one made from its form at boot
(`from_form`, not public: decision 10 holds). Each input becomes the entry
that draws its value as the read-only form did (the guide's table), so the
panel has one read-only renderer; `ReadOnlyComponents` is gone. An entry that
would not fit its field falls back to `TextEntry`, and the made infolist is
not put through the boot checks above: the form's checks hold (a field may
appear twice, a condition may name a hidden input), so no application fails
at boot.

- The view schema carries `infolist` only with a key (a record's view page);
  lists, imports and relation sections ask without one. The form is still
  sent: the dashboard reads it.
- Entries of repeater rows made from inputs carry their row value's kind, so
  dates in rows show as dates.
- One visible change: a `belongs_to` select shows a `RelationEntry`, whose
  title comes with the record (no request per value) and links when the user
  may list the related resource.
- **Conditions.** A made infolist carries `fromForm: true` (vocabulary 17).
  Its conditions read fields as the edit form does: the values the form
  would hold (empty for fields not sent, such as write-only ones), `null`
  where the form's own conditions hide a field, each field with its kind.
  A declared infolist keeps the rule above (conditions on the stored values),
  taking from the form only the kinds of fields it does not show; its
  meaning does not change.
- No entry for an encrypted field (not even whether it is set: the
  read-only form printed its masked state).
- Contributed by @giangittb112000 (PR #3), ported onto 0.13.0 and amended.
