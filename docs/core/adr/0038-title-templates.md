# ADR 0038: Record titles from a template

- Status: accepted (unreleased)
- Date: 2026-11-15

## Context

A record's title is one field (`ResourceMeta::title("name")`). It is used:

- in envelopes (`title`, `engine/adapter/read.rs`): the record page heading,
  the breadcrumb, row menus, dashboard lists;
- by `lookup` (`engine/adapter/operations.rs`): options of `belongs_to`
  selects and their search, ordered by the title column;
- by `titles`: labels of relationship values in lists and records.

Records often share a name: a subject per grade, a product per variant, a
person per id. A select listing "Math", "Math", "Math" cannot be used.

Audit entries store the subject's key, not a title; they are not affected.

## Decision

- `ResourceMeta::title_template("{name} {grade}")`: text with `{field}`
  placeholders. `title("name")` stays and means `title_template("{name}")`.
- **Rendering:** each placeholder is replaced by the field's value as text
  (as `title_text` does today; `null` is empty); runs of spaces are
  collapsed and the result trimmed; an empty result is no title (the SPA
  shows `#key`, as today).
- **Readability:** a title is produced only when the user may read every
  field of the template (as today for the single field); otherwise no title
  (the SPA shows `#key`). Rendering only the readable parts was rejected:
  the template's literal text would be left dangling (`Math (grade )`), and
  marking which text belongs to which field needs more syntax. Applications
  should build titles from fields every user of the panel may read.
- **Search and order** (`lookup`): the search is split into words (repeated
  words count once); each word must match one field of the template (fields
  OR-ed per word, cast to text as today; words AND-ed): "Math 10" finds
  "Math (grade 10)". A single-field title behaves the same, so a search of
  several words no longer matches only the exact phrase. Options are ordered by the
  template's fields in order, then the key.
- **Boot checks:** placeholders name fields of the resource; braces are
  balanced; no encrypted field (as for `title` today); at least one
  placeholder.
- `ResourceInfo.title_field` keeps the single field (`title(...)`, or a
  template of one field); for a template with several fields it is absent
  and a new `title_fields` lists them (absent otherwise). The SPA only shows envelope titles: no vocabulary
  change.

## Consequences

- Every place that shows a record's title gets the template at once
  (headings, breadcrumbs, selects, relationship labels, row menus).
- `lookup` searches several columns: a little more work per keystroke,
  bounded by the existing limits.
- Applications reading `ResourceInfo.title_field` see `None` for multi-field
  templates and should read `title_fields`.
