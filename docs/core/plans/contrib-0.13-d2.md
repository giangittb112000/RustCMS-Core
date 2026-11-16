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
