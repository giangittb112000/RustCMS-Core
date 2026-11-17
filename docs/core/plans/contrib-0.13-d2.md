# Contrib 0.13 — milestone D2 fix design (relationship repeaters)

Input to the Codex design review required by `contrib-0.13.md` before coding D2. Code refs at 612f1a00 (`git show 612f1a00:<path>`): `engine/adapter/rows.rs`, `engine/adapter/operations.rs` (create 453–493, update 526–562), `engine/adapter/many.rs` (`lock_links` 492), `engine/registry.rs` (repeater boot checks ~370, `check_lock_order` ~423).

## Locking (3.8, 3.9, concurrency)

Today a parent write locks: the parent row → its belongs-to-many link targets (`lock_links`, by `lock_rank`) → child rows (`write_rows_in`: named rows by key through `locked_visible`, then `reorder_rows` locks every live child with `ORDER BY pk FOR UPDATE`). Child rows' own writes (update/create through the child adapter) may lock the child's link targets too.

Design:

1. **Children lock their parent** (boot): a relationship repeater requires its child resource to declare `lock_parent` through **this** relation's field (today only a *different* `lock_parent` is refused). Then every child write — direct create, update that moves it, repeater row — locks the parent row first, so writes to one parent's children are serialized by the parent row, and the lock graph (`check_lock_order`) already holds the parent → child edge (parent_lock edges are in it).
2. **Supported configurations** (boot, refused otherwise with a message naming both):
   - the lock closure of a repeater's child (the child and everything it can lock: its `lock_parent` children, belongs-to-many targets, transitively, from the lock graph) shares no resource with the closure of the parent's own belongs-to-many targets, nor with the closure of another relationship repeater of the same parent.
   This rules out one resource being locked twice in one parent write by two phases (e.g. `orders` ↔ `tags` and `order_lines` ↔ `tags`), where keys would not be locked in key order.
3. **Phases in rank order**: after the parent row, the parent write runs its lock phases — one per link-target resource (`lock_links`) and one per repeater child (rows) — ordered by `Registry::lock_rank` of their resource. `lock_links` gains a filter (the target resources of this phase); repeaters of children ranked before a link target run before it.
4. **Rows of one repeater, locked once**: the candidate keys are the named rows (updates, deletes) plus, when `order` is sent, the parent's live children. Discovery is bounded: `COUNT(*)` of the parent's live children first; above `MAX_ROW_ITEMS` (1000, the order's cap) → 422 `row_order` before loading or locking anything. Then one `locked_visible` call (the existing helper: scope + view policy, key order, backend-correct locking, recheck under the lock) over all candidates; rows not visible are not locked and keep their positions. No second lock pass (`reorder_rows` uses the rows locked here).
5. **Positions**: `largest_position` becomes `SELECT MAX(order_column)` over the parent's children **including trashed** (their places stay taken), computed only when rows are created or moved; used for created rows and as the base of the temporary positions of the two-pass reorder. Arithmetic is checked (`checked_add`); overflow → 422. Reads and writes use the column's kind (`i32` number, `i64` string on the wire; 3.12).

## Disclosure (3.7)

`rows_changed` in the parent's history: by default **counts only** (`{"created": 2, "updated": 1, "deleted": 0, "reordered": true}`). Keys are listed only for rows that currently pass the child's scope and VIEW policy for the reader (one bounded `keys_in_scope` + policy check per history page, as link meta does); deleted or not found rows are never listed (fail closed).

## Row rules (3.11)

- Row inputs **inherit** the child form's rules (they are validated by the child's `accept_input`). Boot refuses a row input declaring options, `min`/`max`, `required` or `max_length` that differ from the child form's input of the same field (no second rule set).
- `.addable(false)` / `.deletable(false)` are enforced on the server: a `create` / `delete` part sent anyway → 422 at `relation` ("rows cannot be added/removed"), checked after admission (abilities) and before any key is read.

## Smaller items

- 3.12 i64 positions, see 5.
- 3.13 (SPA) each row renders with the child's context: child slug, the row's key for stored rows, a create context (no key) for new rows; inherited `optionsPath` cleared; row error paths kept.
- 3.14 rows have two schemas: `rowCreate` (child Create components, defaults) for new rows and `rowEdit` for stored rows → contract change (vocabulary 16): types, fixtures, protocol, renderer in the same commit.
- 3.16 `find_model_in(...).ok()` → keep "not one of the rows" for not found, return other errors; a new row's ref must not equal a child key (any kind) nor another ref → 422 at `relation.ref`.
- 3.15 regression tests: per-row policy refusing update/delete, field rules on row inputs, media in rows, `min_items` on delete, the boot refusals above, the panel's "removing a row keeps an earlier move", concurrency (direct child create vs repeater `max_items` and positions) on PostgreSQL and MySQL.

## Questions for Codex

1. Is requiring `lock_parent` (1) + the closure rule (2) + rank-ordered phases (3) enough to make every parent write lock in the global order? What else locks inside a row write (child hooks, media references, belongs-to checks with shared locks) that could still invert?
2. Does one `locked_visible` call over all candidates (4) give correct semantics on MySQL and PostgreSQL (rows moved meanwhile cannot happen thanks to the parent lock — right?), and is skipping invisible rows safe for the two-pass reorder with a unique `(parent, position)`?
3. Anything wrong in the history (3.7) and row-rule (3.11) choices?

## v2 (after the Codex design review): a supported subset, refused otherwise

Codex did not approve v1: ordering phase roots does not order whole phases, and rows take locks the graph does not show (belongs-to references, media references per row, hooks, cascades). v2 keeps the write path and **narrows what boots** so that a parent write locks only: its own parents (as today), its row, its links (as today), then the rows of its repeaters' children — nothing else.

Boot refuses a relationship repeater unless:
1. the child declares `lock_parent` through this relation's field (every child write — direct create, move, repeater row — locks the parent row first: writes to one parent's rows are serialized by that row);
2. the row inputs are plain fields of the child: no `belongs_to` field (other than none: the parent key is set by the server), no media field, no rich text with media, no encrypted field, no belongs-to-many (row writes then take no lock on another resource through references);
3. the child has no belongs-to-many relation and no `lock_parent` children of its own (its writes lock nothing below it);
4. the parent's own belongs-to-many targets are not the child (no resource locked twice).
Hooks of the child that write other resources, and database cascades from the child, are outside what is supported (guide, ADR 0040).

Within that subset:
- **One pass over the rows**: the named rows (updates, deletes) are pre-filtered by parent membership in the query, then, when `order` is sent, the parent's live children are discovered with `LIMIT 1001` (beyond 1000: 422 `row_order` before any lock). All candidates are locked once through `locked_visible` (key order, scope + view policy, recheck under the lock); rows not visible are not locked and keep their positions. Reorder uses the surviving locked rows plus the rows created by this write.
- **Positions**: `MAX(order_column)` over all children (trashed included), after the creates, checked arithmetic within the column's range (`i32`/`i64`, read and written by its kind); overflow → 422.
- **History** (`rows_changed`): counts only, keys never (fail closed).
- **Rules**: row inputs inherit the child form's rules; boot refuses a row input that declares options, bounds, `required` or `max_length` of its own. `addable(false)`/`deletable(false)` are enforced by the server (422 at the relation), after admission.
- **Refs**: new rows are named in `order` as `{"new": ref}` (tagged), so a ref never mixes with a key; `create` keeps `[ref, data]`. Contract change with the SPA.
- **SPA**: rows render with the child's context (child slug, row key or none); new rows use the child's Create components and stored rows its Edit components (`rowCreate`/`rowEdit`, vocabulary 16).

## Codex round 2 on v2 (2026-11-17): not approved

Narrowing the repeater's configuration is not enough: deadlocks remain through paths the repeater does not own — the child's other references written by direct updates or restores, the parent's own references (e.g. a self-referencing FK while a child moves between parents), hooks adding references, database cascades, and repeater phases not ordered by `lock_rank` among link targets. A correct D2 needs a lock contract for the whole write path (every reference lock planned with the resource locks), not a repeater-local fix.

Decision needed (owner): (a) design that lock contract (a Core change, its own ADR), then finish relationship repeaters on it; or (b) remove `.relationship()` from the integration and release JSON repeaters, counts and history first.

## v3 (2026-11-17, after the owner chose to finish relationship repeaters): the contract Core already has

Fact missed by v1/v2: Core does not order **implicit** locks anywhere. FK enforcement on INSERT/UPDATE, cascades and hook writes take locks no writer plans, on every write path today; the backstop is `From<DbErr> for AppError` (`crates/rustcms-core/src/error.rs:168`): deadlocks, lock-wait timeouts and serialization failures roll the write back and answer **409 `write_conflict`** (nothing applied, retryable). ADRs 0018/0020/0035 order the **explicit** locks (`lock_parent`, `lock_visible`, link targets, `media::check_references` shares). v3 makes relationship repeaters follow exactly that contract, instead of a stronger one no other write has.

### Contract (ADR 0041)

1. **Explicit locks in one order.** A write takes its explicit locks in `Registry::lock_rank` order of their resources, keys in key order within a resource, and never returns to a resource of lower rank. A parent write with repeaters: its parents (`lock_parents`, as today) → its row → then, by rank, each link-target resource (`lock_links`, filtered per phase) and each repeater child resource (one locking pass, below). Two repeaters on the same child resource are refused at boot (one phase per resource).
2. **The family serializes on the parent row.** A repeater's child must declare `lock_parent` through that relation (boot): every child write (direct create, update that moves it, delete, restore, repeater row) locks the parent row before its own; parent writes lock it first too. Moves lock both parents in key order (`lock_parents`, as today).
3. **No explicit lock outside the plan inside a row.** Row inputs may not be media fields, rich text (may hold media), belongs-to-many, encrypted, nor `belongs_to` fields (boot): a row write's only explicit locks are its own row (taken in phase) and the parent (already held). The parent key and the order column are set by the server.
4. **Implicit locks**: FK checks by the database, cascades, hook writes — as everywhere in Core: not ordered; a cycle is detected by the database and answered 409 `write_conflict` (the parent and every row rolled back). Documented in ADR 0041 and the guide (hooks of a repeater's child should not write other resources).

### Within the contract (unchanged from v1/v2, accepted by Codex)
- One locking pass per child phase: named rows pre-filtered by parent membership in the query; with `order`, the parent's live children discovered with `LIMIT 1001` (beyond: 422 `row_order`), all candidates locked once by `locked_visible`; survivors = locked visible rows not deleted, plus rows created by this write.
- Positions: `MAX(order_column)` over all children incl. trashed, taken after the creates (the parent lock excludes concurrent creates); temporary positions above it; checked arithmetic within the column's range; read/written by the column's kind (`i32`/`i64`); overflow → 422.
- History `rows_changed`: counts only (fail closed).
- Row rules inherit the child form's; boot refuses row inputs with their own options/bounds/required/max_length; `addable(false)`/`deletable(false)` enforced by the server after admission (422 at the relation).
- Refs tagged in `order` (`{"new": ref}`): exact shape, refs unique and known, keys canonical, duplicates after resolution refused.
- SPA: rows render with the child's context; `rowCreate`/`rowEdit` schemas (vocabulary 16).

### Tests
Boot refusals (no `lock_parent`, wrong relation, two repeaters on one child, each forbidden row input kind); concurrency on PostgreSQL and MySQL: a direct child create vs a repeater save with `max_items` (serialized on the parent: never more than max), a reorder vs a direct child update (no deadlock, both applied or one 409), a move between parents vs a repeater save; positions with i64 and overflow; tagged refs; history counts only; addable/deletable refused.

## v4 (Codex round 3 on v3: direction accepted, gaps fixed)

The contract is about **new explicit lock acquisitions** (re-entering a held parent lock is not one); implicit locks (database FK enforcement, cascades, hook DML) stay outside it, as everywhere in Core, behind 409 `write_conflict` (the error aborts the operation and the transaction is dropped/rolled back; MySQL lock-wait timeouts included).

1. **Media locks last, on every create/update (a Core fix, not only repeaters).** `check_written` is split: reference reads (`check_relations`, no explicit locks) stay where they are; `media::check_references` (explicit shared locks on media rows) moves after every other explicit lock of the write: after `lock_links`, and for a parent with repeaters after its child phases. The media resource is outside the lock graph, so it ranks last (`lock_rank`), which makes "media last" the rank order. Applies to resources without repeaters too (parent → media → link targets was already out of order).
2. **Link targets before repeater children.** For each relationship repeater (P, C) and each belongs-to-many target T of P, the lock graph gains an edge T → C. Then the parent write's order — parents, row (update: locked; create: prelocked links before the insert), link targets (`lock_links`, as today, before the row write), children (after the row write), media — is rank order. A cycle (e.g. C linking to T) is refused at boot by the existing cycle check. Repeaters of one parent run in rank order of their children; two repeaters on one child resource are refused.
3. **Rows take no explicit lock but their own.** Boot refuses row inputs whose child field is media, rich text, encrypted, belongs-to-many or `belongs_to` (from the child's field metadata, exposed through `DynResource`). After the child's hooks, a row's data holding a media or rich-text field is refused (500 naming the hook: "hooks of a relationship repeater's child may not write media fields"), so no media lock is taken inside rows.
4. `find_model_in(...).ok()` → only `NotFound` becomes "not one of the rows"; other errors propagate.
5. **Bounds**: `min_items`/`max_items` hold for saves through the repeater (checked under the parent lock, so concurrent repeater saves cannot pass them). Direct child writes (relation section, API) are not bounded by the parent's repeater (guide, ADR); tests assert exactly that.
6. Tests add: a parent with media and links and a repeater (order of phases), a hook adding media in a row (refused), SQLite busy → 409 (exists: `rustcms-migration/tests/databases.rs`), PostgreSQL/MySQL concurrency (repeater save vs direct child write/move: no hang; either both apply or one 409).
