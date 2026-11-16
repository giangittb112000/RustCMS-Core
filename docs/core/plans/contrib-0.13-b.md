# Contrib 0.13 — milestone B fix design (conditions, title templates)

Input to the Codex design review required by `contrib-0.13.md` before coding B. Code refs at 0c3d9fd.

## 2.1 + 2.4 — the condition projection

The SPA evaluates conditions on `wireValues(inputsOf(components), values)` (`admin-ui/src/schema/form.ts:224`): only the inputs of **the user's form schema for this operation**, with values from `defaultValues` (`form.ts:202`: the record's data on edit, the schema's `default` on create) plus the draft. The server must evaluate on the same thing, not on the stored row.

`ResourceAdapter::condition_values(ctx, op, stored: Option<(&Data, viewable: bool)>, input: &Data, bound: Option<(&str, &Value)>) -> Data`:

1. For each field with an input that `input_schema` would include for this user and op (create: `can_write(Create)` and key rule; edit: `can_read || can_write(Update)`, key excluded):
   - create: the column default when the schema sends it (`can_read`, i.e. not write-only: `schema.rs:224`), else nothing;
   - update: the stored value when the record is viewable **and** the field is readable and not a secret; else nothing (write-only fields, blind writers: the SPA has no value);
2. overlay every accepted input value (`input` after `accept_input`; only writable fields survive it);
3. the bound key (relation-manager create, `ctx.bound`): its binding value. The SPA gets it in its evaluation context only (the record page passes `{[field]: parentKey}` into the values it evaluates; it is never posted: `operations.rs:425`).
4. Everything else is absent (= `null`). Hidden-dependency-as-null stays as in both evaluators (`conditions.rs:85`, `conditions.ts:84`).

`viewable` = `policy.allows(VIEW, Some(&record))` on the locked record (update path, `operations.rs:503-510`), computed before the call (`apply_conditions` stays sync).

What the projection drives: which fields are hidden (dropping their input), `clear_when_hidden` writes (still only for `can_write`), `required` when shown. What is written stays the real input. Every outcome then depends only on values the user already has → no oracle (unreadable field, guarded field, unviewable record).

Bound key (2.4): never removed from the written input by a condition (exempt in step "drop hidden input"); write/type checks and the "a hook changed the relation key" check unchanged.

## 2.6 — required when shown (transitions)

`hidden_before` = hidden on the projection **without** the overlay (stored/defaults as the user saw them), `hidden_after` = with it. A shown, required, writable field with a null resulting value is an error when: create; or the field is in the input; or it is in `hidden_before` and not in `hidden_after`. A plain PATCH omitting a field that was already shown and null keeps today's behaviour (no error).

## 2.3 — clear_when_hidden

Write the cleared value when the field is hidden, the user may write it, and the **stored** value (update) differs from the cleared value — compared with the locked row, not with the merged input. On create: the cleared value replaces any sent value (the field is hidden) — already the case since the input is dropped; insert the cleared value when it differs from the column default.

## 2.7 — null on a conditional input

`accept_input` currently rejects `null` on a non-nullable conditional input (`validate.rs:102`) before visibility is known. For conditional inputs: skip the `required` and non-null checks in `accept_input`; `apply_conditions` then drops the value if hidden, or reports `is required` (shown, required or non-nullable, value null) before hooks.

## 2.2 — settings and kept_while

In `features/settings/http.rs` compute `final_values` (stored ⊕ sent, minus hidden sent values, plus resets of hidden `clear_when_hidden` fields to their defaults) first; `changed(name)` = `final_values[name] != stored-or-default[name]`; use it for `kept_while` and for the media reference check (`http.rs:397,455`).

## 2.5 — canonical conditions in schemas

`FormConditions::new` already canonicalizes literals when checking (`check`). Keep the checked tree per input/layout (by component path) and emit it in schemas (resource forms, layouts, settings pages, action forms) instead of the declared one. Cases added to `fixtures/conditions/cases.json`: unhyphenated/upper-case UUID literal vs stored canonical value for `eq`, `ne`, `is_in`, nested in `all`/`any`.

## 2.8 — tabs

`TabsLayout` keeps the active tab as the index in the **declared** tab list (before filtering by conditions). When the active tab becomes hidden, show the first shown tab without changing the stored choice; when it is shown again it is active again. Keyboard navigation moves among shown tabs; error focus opens the tab holding the first error (by declared index).

## 2.9 — docs and search

ADR 0038: search is per word, each word matched (OR) across the title fields, every word required (AND). CHANGELOG/upgrade: `title(...)` search behaviour change, empty titles omitted from `titles()`, `rustcms_admin::Condition` may clash with `sea_orm::Condition` under glob imports. Search words are deduplicated (no cap). Kitchen-sink gets one conditional input and one conditional section so `fixtures/contract` carries `shownWhen`/`clearWhenHidden`.

## Tests (each with a reverse check)

- 2.1: HR-only `salary_band`; `bonus.shown_when(salary_band = C).required().clear_when_hidden()`; a non-HR editor: PATCH `bonus` → same outcome whatever `salary_band` is; empty PATCH → no `required`; unrelated edit → no clearing difference. Same with a field-readable but record-unviewable user (policy VIEW false, UPDATE true). Blind writer sending the dependency sees the dependent field follow the value sent.
- 2.4: relation-manager create with the bound FK under a hidden condition → 201, FK kept; a condition reading the bound FK evaluates the bound value (Rust + SPA parity case).
- 2.2, 2.3, 2.5, 2.6, 2.7 as listed in `contrib-0.13.md`; 2.8 vitest.

## Revisions after the Codex design review

1. **Projection = the SPA's form values on the wire.** For each input of the user's schema (edit includes a readable primary key, disabled), the available value goes through the same empty conversion as the SPA (`toFormValue`/`fromFormValue`): no value → `false` for a non-nullable toggle/checkbox, `""` for non-nullable text kinds, `null` otherwise. Implemented once (`condition_values`) for resources; settings and action forms keep their own evaluators with the same empty rule.
2. **Relation-manager create (SPA):** instead of removing the bound input (`withoutInput`), the page turns it into a disabled `hidden` input whose default is the parent key. Its kind and conditions stay in the schema; renderer, resolver and payload see the same value; disabled inputs are never posted. Chained conditions where the bound field itself is hidden follow the usual rule (null).
3. **clear_when_hidden without an oracle:** when the field is hidden and the user may write it, the cleared value is **always** inserted (no comparison with the stored value), as a trusted server value after input validation (like a hook's value: not checked against the field's input rules). Writing an unchanged value is harmless (audit diffs skip equal values).
4. **2.6 + 2.7 reconciled.** Presence = the name is in the input (explicit `null` is present). For a conditional field, `accept_input` skips the required and non-null checks. In `apply_conditions`, a shown, writable field is `is required` when (required or non-nullable) and: present with `null`; or create with a null projected value; or update with a hidden → shown transition (projection without vs with the overlay) and a null projected value (a write-only field revealed must then be sent). Omitted and already shown: no check (today's PATCH behaviour).
5. **Settings 2.7:** the settings write skips the null/required check for conditional fields before visibility, then checks shown ones. The values used to evaluate conditions (hidden dependency = null) stay separate from the final values of 2.2.
6. **2.5 concretely:** at boot, the checked (canonical) condition is written back **into the stored form components** (resource forms, settings forms, action field inputs) by one in-place pass, so every schema emits canonical literals without path bookkeeping. SPA canonicalizes UUID draft values like the server (lower case, hyphenated from 32 hex digits). Tests: emitted-schema assertions (UUID literal canonical in the schema JSON), fixture cases with non-canonical draft values; the Rust fixture mapper gains UUID.
7. **Parity tests** cover: create defaults/placeholders (non-nullable text and toggles), readable disabled key, blind and write-only edits, relation binding, CSV create rows and matched update rows, settings and action forms. Concurrent edits: the server decides on the locked row (documented in ADR 0037).

## After Codex verify (d30f0e7)

- `""` in a nullable text input (or a non-text kind) is no value in the projection, as the panel's round trip makes it; settings too.
- A field with several inputs: the last input wins (as `defaultValues`/`wireValues`).
- Settings reset a `clear_when_hidden` field only when the save hides it (same rule as resources).
- Kept on purpose: the create and hidden→shown "required" checks stay on `required` fields only. Every conditional non-nullable column has a default (boot check), so an omitted value is stored as that default; requiring it would refuse writes the database accepts. A shown non-nullable field sent as `null` is still refused.
