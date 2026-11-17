# Plan: 0.13.1 — PR #3's later parts (view page, rich text v3, form reactions)

Status: draft v1 (2026-11-17). Owner decision: take all three, fixed by us, into 0.13.1.
Branch `review/pr3-0.13.1` (worktree `../RustCMS-pr3`) from `main` after v0.13.0.
Source: contributor commits on `pr3-new` (PR #3 head): view page 06cbfad..de99250, reactions b0c1277,
rich text 7705bb2. Reviews: three Claude agents + Codex (findings below, ids V*, R*, F*).

Order (each part: port, fix, tests, docs, Codex verify, then the next):
1. **Default view page** — no vocabulary change.
2. **Rich text v3** — ADR 0042, vocabulary 17, document version 3, SDK `ui` 4.
3. **Form reactions** — ADR 0043, vocabulary 18.

Porting is by hand on top of 0.13.0 (cherry-pick conflicts in infolist/form already at the second commit);
authorship kept with `Co-authored-by: giangittb112000` and CHANGELOG credit.

## 1. Default view page

Port: `from_form`/`form_entry`/`entries_of`, `Entry.row_kind` (main's `row_components` hard-codes `"json"`;
`engine::repeater::row_kind` exists), operations.rs `(Some(record), View)` match, mod.rs: call `from_form` after
the form's conditions are canonicalized. SPA: remove `ReadOnlyComponents`/`RelationValue`, keep main's
`TabsLayout` `shown` prop (do not bring back `withShownChildren`), keep main's decimals clamp, `bindVia` untouched.
Drop parts already on main (`isLayout` fix, rows get slug/recordKey: main 8c10086). Convert tests still using
`ReadOnlyComponents` (read-only, conditions, editor-layout, repeater) to `tests/view.ts`.

Fixes:
- **V1 (major)**: the form's hidden→null rule applies only to the **generated** infolist (its entries carry the
  inputs' conditions); a declared infolist keeps ADR 0039 (conditions read stored values), taking only the kinds
  from the form. Test both.
- V2: generated view reads absent inputs as the edit form does (form projection: empty form value for
  write-only/unreadable inputs, `""` → null for nullable text), or the docs stop claiming "as the edit form";
  choose projection (shared helper with the form).
- V3: React keys `name + index` (duplicate field twice at one level → console error fails `watch` specs).
- V4: `tests/view.ts` mirrors the server's fallbacks (`file_upload` on a non-media field, `rich_editor` on a
  non-rich field → text); the schema carries what it needs, or document the gap in the test.
- V5: no "plan branch N" in public source comments.
- Docs: ADR 0039 amendment (default view page; vocabulary stays 16), upgrade.md "From 0.13.0 to 0.13.1": keyless
  `operation=view` no longer returns `infolist`; encrypted fields' set/not-set no longer shown on view pages;
  relation values render as links. Guide infolists (default view page, input→entry table), forms.
- Tests: `db_test!` that encrypted/encrypted_map/guarded/write-only fields have no generated entry and no schema
  entry, with a non-admin user (per-user `readable` rule); `rustcms_testing::create_table`; reverse checks.
  SPA parity test on fixtures. E2E view-pages with cleanup in `finally`.

## 2. Rich text v3 (ADR 0042, vocabulary 17, document version 3, SDK ui 4)

Port: `features/rich_text/*` and SPA rich text files almost clean (main untouched since base); re-apply
`rich_nodes` to every `InputRule` construction in main's `engine/adapter/input.rs` and `validate.rs`, the fill
step in `apply`, `parse_with` in `check_media`. Strip the reactions context lines from the contributor's base.
SDK copies (`admin-ui/sdk` → `crates/rustcms-cli/templates/sdk`, `examples/demo/resources/admin/sdk`).

Fixes:
- **R1 (high)**: byte budget on the **completed** document: account for defaults while parsing (refuse before
  allocating a large expansion), re-check after `fill` and in `AppNodes::complete`; boot caps the total size of a
  declaration's defaults. Test: 581-byte submission that expanded to 301 KB is refused (422), stored documents
  stay parseable.
- **R2**: block atoms refused anywhere inside a callout (nested lists included), server and SPA `asDocument`.
- **R3**: attribute name `node` reserved at boot (duplicate `data-node`, editor marker overwrite).
- **R4**: server stores the lowest version a document needs (lower `version` in the same pass as `fill`), so
  API/import/hook writes keep older readers working (ADR 0008).
- **R5**: undeclared/removed elements: the editor keeps the document editable; such atoms are opaque, can be
  deleted, never edited; the server accepts a write that only removes them. Same when a declaration gains a
  required attribute or tightens bounds (the atom is shown as invalid and can be removed or fixed).
- R6: `Attr::label(..)` translated like node labels; generated attribute form uses it (fallback: name).
- R7: view-page fallback for a legacy non-document string keeps showing it raw.
- R8: E2E cleanup in `finally`.
- R9: ADR notes: integer attribute values and `min/max` are JSON numbers bounded to ±(2^53−1) (exception to
  ADR 0003, stated in the protocol); SPA outline ids may differ from `Html::outline`; `Attr::uuid` is not a media
  reference (no visibility/reference check). Error messages truncate user-supplied names.
- Actions writing rich text through the ORM are trusted writes (as today for every field rule): documented, not
  enforced (Codex 6).
- Docs: ADR 0042 (numbers fixed: 0.10–0.13 readers), ADR 0008 amendment "(0.13.1)", guide media/forms (samples
  by `#region` from checked code), protocol, error codes, upgrade.md (`Mark` `#[non_exhaustive]` with new
  variants, `parse` refuses `app.*`, SDK `ui` 4, rollback: 0.13.0 refuses v3 documents).
- Tests: server on 3 databases (bounds, callout nesting, `node`, version lowering, removal of unknown atoms),
  SPA, E2E.

## 3. Form reactions (ADR 0043, vocabulary 18)

Port onto main's `condition_values(ctx, op, stored.map(|r| (r, viewable)))` and bound-key handling (ADR 0037).

Design changes:
- **`.strict()` is removed** from 0.13.1. It ran application code inside the write transaction with locks held,
  outside the reaction limits (F-M1, Codex 8), was bypassed by relationship rows and actions (F-M2, Codex 3, 6),
  could fail forever on non-canonical patches (Codex 9) and on records saved before the reaction existed, and
  could not be satisfied by API clients (token actors get 403 on `form/react`) or relation dialogs (F-M3).
  Invariants belong in `mutate_before_*`/validation; the guide says so. A later ADR may add a server-side,
  bounded consistency rule.
- **F-B1 (blocker)**: reaction state = the user's form: base `condition_values` (unreadable/guarded fields and
  blind edits → empty form values; stored values only with VIEW), overlay submitted values **only for inputs the
  user may write**; readable-but-not-writable fields keep the stored value (with VIEW) or empty. `hidden` is
  computed on that map. Tests: update-without-view, unreadable controlling field, each with a reverse check.
- **F-M4**: submitted values for non-writable fields never reach the reaction (above); `belongs_to`/media keys in
  the state are checked against scope as a save would (or dropped); guide warns that state values are user input.
- **Bounds (Codex 4)**: submitted repeater rows ≤ the input's `max_items` (≤ 100) before the callback; patches ≤
  200 per answer; `notice`/`flag_row` text ≤ 500 chars; row patches applied to one mutable array per repeater
  (no clone per patch); filtering counted in the time budget (reaction timeout covers callback + filtering).
- **Codex 10**: visibility recomputed against the evolving state before each patch.
- **Canonical patches (ADR 0003)**: each kept value goes through the input's acceptance (decimal scale, select
  canonical form, i64 as string, rows through `rows.check`, `add_row` sends the checked row).
- **Admission (Codex 7, minor)**: CREATE-or-UPDATE admitted and the rate limit reserved **before** the body is
  read (same 403 whatever the input); then operation/record checks (scope, trash, UPDATE; VIEW decides stored
  values).
- **Concurrency**: per-user 2 + global 8 (one user cannot hold every permit); the SPA only needs the latest answer
  (abort previous request).
- **Relation-create (Codex 11)**: the request carries the relation context; the server authorizes it as the save
  does and puts the bound key into the state (main's `bindVia`).
- **Rich editor (Codex 12)**: external value changes synchronize into TipTap without emitting a user edit.
- **F-M5 demo**: `FormState::previous(name)` (value before the change) so the demo fills the slug when it is empty
  or still equals the slug of the previous name; E2E types with a pause.
- Row flags keyed by row key/ref, not index. Reactions documented read-only (ctx is not a transaction).
- Child row inputs never marked `reacts` (relationship rows do not run reactions); boot refuses `reacts` on a
  relationship repeater's child inputs used as rows, or the schema drops it.
- Route in `rustcms_admin::http::routes`, cases in `fixtures/authz/admin.toml` (oracle), new messages in
  `i18n::TEMPLATES`.
- Docs: ADR 0043, guide forms "Reacting to changes" (state is user input; read-only; no strict), protocol, error
  codes, upgrade.md, CHANGELOG (credit).
- Tests: server on 3 databases (authz oracle, blind edit, unreadable condition, non-writable overlay, bounds and
  timing with 10,000 submitted rows refused fast, canonical patches, evolving visibility, relation-create bound
  key), SPA (debounce/abort, row flags by key, rich editor sync), E2E with `watch` (slug with pauses).

## Gates and release

Per part: `just test-fast`, lint, `UPDATE_CONTRACT`/TS types reviewed, `just admin-build` (assets committed
separately), Codex verify. At the end: `just ci`, `just test-matrix` (or CI when Docker is unavailable),
`just generated`, `just docs`, `just e2e`, `just test-cli`, `npm audit`. Release 0.13.1 per
`docs/core/guides/releasing.md` (upgrade.md "From 0.13.0 to 0.13.1", release notes, SDK VERSION ×3), tag after CI
green, notify RDockPanel, comment on PR #3 and close it.
