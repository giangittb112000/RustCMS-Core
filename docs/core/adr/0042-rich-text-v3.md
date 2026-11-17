# ADR 0042: Rich text version 3: new marks and application nodes

- Status: accepted (0.13.1, document version 3, vocabulary 18, SDK ui 4); contributed by @giangittb112000 (PR #3), ported onto 0.13.0 and amended (see the end)
- Date: 2026-11-17
- Amends: ADR 0008 (media and rich text)

## Context

Rich text (ADR 0008) is a closed, versioned JSON document, validated and
bounded on the server, never HTML. Version 2 added tables, embeds, callouts,
image captions and widths. Two needs remain:

- **Common formatting:** underline, superscript (m², 1st) and subscript (H₂O).
  These are needed by blogs, course content and documentation alike.
- **Elements an application defines.** Some cases cannot be covered by a
  closed set of nodes:
  - a blank in a fill-in-the-blank question;
  - a product card in a shop's article;
  - a mention, or a keyboard shortcut.

  Today the only options are text conventions (`{{1}}`), which the editor
  cannot show or check, or a separate field.

Constraints from the existing design:

- **The server decides.** `rich_text::parse` runs on every write path (form,
  API, CSV import) through the field's rule (`engine/validate.rs`).
- **Never HTML, never remote content, no CSP change** (ADR 0008, 0019).
- **Older readers keep working.**
  - The editor writes the lowest version a document needs.
  - A panel that does not know a version shows the document read-only and
    never rewrites it.
- **Extensions cannot bring TipTap code** (ADR 0032).
  - Host modules are React, React Query and the `ui` set. Everything else
    is bundled into the extension.
  - A second copy of `@tiptap/core`/ProseMirror inside an extension would
    build node types the panel's editor does not recognize.
  - Exposing TipTap as a host module would make every TipTap upgrade a
    breaking change for extensions.
- `parse`, `to_html`, `to_html_with` and `outline` are free public
  functions used by headless consumers; they know no application
  configuration.
- `Mark` is not `#[non_exhaustive]`.

## Decision

### Version 3

- `VERSION = 3`.
  - Versions 1 and 2 are read as they are: 1 ⊂ 2 ⊂ 3, and no node or
    attribute changes meaning.
  - `parse` refuses version 3 parts in an older document: a document never
    claims less than it uses.
- Version 3 parts are:
  - the marks `underline`, `superscript` and `subscript`;
  - application nodes (`app.*`).
- The editor writes the lowest version the content needs: 1, 2 or 3.
- `upgrade_value` still lifts version 1 documents to version 2 only (only
  `version` changes). It never writes version 3: a document is in the lowest
  version it needs, so readers of 0.10 to 0.13.0 (a rollback, another reader of
  the same database) keep reading documents without version 3 parts.
- The version message becomes "is not a valid document (version 1 to 3)".
  New "needs document version 3" messages are added with their templates
  (code `document_needs_version`).

### Marks

- `Mark::{Underline, Superscript, Subscript}`, without attributes.
- `Mark` becomes `#[non_exhaustive]`. This is a breaking change for code
  matching it exhaustively; the CHANGELOG and upgrade notes say so.
- **Superscript and subscript exclude each other** on one text, and the
  server refuses a text with both.
- `MAX_MARKS` goes from 5 to 7, the most a valid text can hold: one of
  sup/sub, plus bold, italic, underline, strike and link. `code` with the
  others stays allowed, as today.
- Editor:
  - underline is StarterKit's own, turned on;
  - superscript and subscript are two small marks of the panel, with no new
    dependency, each excluding the other;
  - toolbar buttons, and shortcuts `Mod-U`, `Mod-.` (superscript) and `Mod-,`
    (subscript).
- HTML: `<u>`, `<sup>` and `<sub>`.

### Application nodes: declaring

```rust
use rustcms::admin::rich_text::{RichTextNode, Attr};

pub fn rich_text_nodes() -> Vec<RichTextNode> {
    vec![
        RichTextNode::inline_atom("app.kbd")
            .label("Keyboard shortcut")
            .attr("keys", Attr::text().max(100))
            .text(|attrs| attrs.text("keys").to_owned()),
        RichTextNode::block_atom("app.product")
            .label("Product")
            .attr("id", Attr::uuid())
            .attr("layout", Attr::one_of(["card", "row"]).default("card")),
    ]
}

// Provider
let mut admin = AdminProvider::new();
for node in rich_text_nodes() { admin = admin.rich_text_node(node); }

// Resource
Field::column(question::Column::Body).rich_text().nodes(["app.blank"])
```

- **Names:** `app.[a-z][a-z0-9_]{0,31}`.
  - Core node names never start with `app.`, so a later core node cannot
    take an application's name.
- **Atoms only:** an inline atom (inside text) or a block atom (a block).
  - Neither holds content, text or marks: only attributes.
  - Nodes that hold content are left for later.
- **Attributes** (`Attr`):

  | Kind | JSON value | Bounds and options |
  |---|---|---|
  | `text()` | string | `.max(n)` characters (default 1,000, at most 10,000); `.min(n)` |
  | `integer()` | number | `.range(min..=max)`, within ±2^53 so it is exact in JavaScript |
  | `boolean()` | `true`/`false` | |
  | `one_of([...])` | string | one of a closed set (at most 50 options) |
  | `uuid()` | string | written in lower case with hyphens |

  - Each attribute is required unless it has `.default(v)`. The default is
    checked against its kind at boot. The server fills in a missing
    attribute with its default before the document is stored, so a later
    change of a default never changes stored documents.
  - Names are `[a-z][a-z0-9_]{0,31}`, at most 16 attributes per node.
  - There is no media, link or HTML kind: an attribute is never markup and
    never an address.
- **`.label(text)`**: shown in the editor's Insert menu and on placeholders,
  and translated like input labels.
- **`.text(f)`**: optional. It is the node's plain text, used in exported
  HTML and in the outline.
  - `f: Fn(&NodeAttrs) -> String + Send + Sync`: synchronous, no `Ctx`, so it
    must be fast and pure.
  - Without it, the text is empty.
- **A field opts in:** `Field::rich_text().nodes([...])` lists the nodes it
  accepts. By default a field accepts none, so a question's blank never
  appears in a blog post.
- **Checked at boot** (panic with the resource and field):
  - a malformed or repeated name;
  - a malformed or repeated attribute;
  - a default or bounds that do not fit the kind;
  - `.nodes` on a field that is not rich text;
  - `.nodes` naming a node that was never declared. This is checked when the
    panel is built, so the order of `rich_text_node` and `resource` calls
    does not matter.

### Application nodes: in documents

```json
{"type": "paragraph", "content": [
  {"type": "text", "text": "Press "},
  {"type": "app.kbd", "attrs": {"keys": "Ctrl+C"}},
  {"type": "text", "text": " to copy."}
]}
```

- `Node::App(AppNode { name, attrs })`, where `attrs` is a map of checked
  JSON scalars.
- **Where they may be:**
  - **Inline atoms** wherever text may be: paragraphs, headings, table
    cells, callouts. They are never in code blocks.
  - **Block atoms** wherever any block may be: the top level, list items and
    blockquotes.
  - Block atoms are never in a callout or a table cell, which hold only
    paragraphs (and lists, for callouts) as today.
- **Valid only when all of these hold:** the document is version 3, the
  field accepts the node, the attributes match the declaration exactly (no
  unknown, missing required or wrong kind), and the node has no `content`,
  `text` or `marks`.
- **Limits:**
  - each application node counts toward `MAX_NODES`;
  - `MAX_APP_NODES = 500` per document;
  - `MAX_BYTES` still bounds the whole document.
- **A node no longer declared, or no longer accepted by the field:**
  - saving the document is refused with "has an unknown element `app.x`";
  - the editor opens the document read-only and never drops the node;
  - the view page shows an "unknown element" placeholder.

  The user removes the node, or the application keeps its declaration while
  documents still use it.
- New messages, with templates in `TEMPLATES` (code `document_element`):
  - "has an unknown element `{node}`";
  - "has an element `{node}` with an invalid attribute `{attr}`";
  - "has an element `{node}` without its attribute `{attr}`";
  - "has an element `{node}` out of place";
  - "may not have more than 500 application elements" (code
    `max_app_nodes`).
- **Public functions:**
  - `rich_text::parse(value)` keeps its signature. It knows no application
    node, so it refuses every `app.*`.
  - `rich_text::AppNodes::new(nodes)` builds the table. Then:
    - `nodes.parse(value)` accepts the table's nodes (every one, with no
      field opt-in);
    - `nodes.parse_for(value, &["app.kbd"])` limits them to a list.
  - The panel builds one table at boot and gives each field's rule its
    accepted names.
- Audit is unchanged: documents are logged by size and digest only.

### Exported HTML and outline

```html
<p>Press <span data-node="app.kbd" data-keys="Ctrl+C">Ctrl+C</span> to copy.</p>
<div data-node="app.product" data-id="0199…" data-layout="card"></div>
```

- An application node becomes:
  - `<span data-node="app.x">` for an inline atom;
  - `<div data-node="app.x">` for a block atom.
- Each attribute becomes `data-{name}`. Names are already restricted to
  `[a-z0-9_]`; values are escaped (booleans and numbers as written).
- The element holds the node's `.text` (escaped). A frontend may replace the
  element by its own rendering, under its own CSP.
- **Never HTML from the application:** this version has no sanitized custom
  rendering.
- `to_html` and `to_html_with` keep their signatures. Without declarations
  they write the element and its `data-*` with no text. Fallback texts come
  from a new builder:

  ```rust
  rich_text::Html::new(&doc).images(&infos).nodes(&app_nodes).render()
  ```

  The two old functions become wrappers of this builder.
- `outline`:
  - An inline atom in a heading contributes its `.text`, so ids follow it
    and stay stable for the same text.
  - Without declarations it contributes nothing.
  - `Html::outline()` uses the same texts as the HTML.

### Schema (vocabulary 18)

- `InputSchema` of a `RichEditor` gains `richNodes`, sent only when the
  field accepts nodes:

  ```json
  "richNodes": [{"name": "app.kbd", "label": "Keyboard shortcut", "inline": true,
    "attrs": [{"name": "keys", "kind": "text", "required": true, "max": 100}]}]
  ```

  Each attribute carries `kind`, `required`, and `default`, `min`, `max`,
  `options` when set.
- `VOCABULARY_VERSION = 17`. A panel built for 16 shows forms read-only, as
  for every vocabulary change. Its `asDocument` already refuses version 3, so
  it shows those documents read-only and never rewrites them.

### Panel

- **Editor:**
  - The panel builds one TipTap node per entry of `richNodes`:
    - an atom, inline or block;
    - selectable and draggable;
    - no `style` attribute.
  - The node is drawn by a React node view (`@tiptap/react`). It uses the
    extension's `view` when one is registered, inside an error boundary.
    Otherwise, or when the view throws, it shows a labelled placeholder
    chip.
  - An **Insert** menu on the toolbar lists the field's nodes. Inserting or
    editing a node opens a dialog with:
    - the extension's `edit` component when there is one;
    - otherwise a form generated from the attribute kinds (text, number,
      switch, select).

    The panel checks the attributes against the declaration before applying
    them.
  - **Pasted HTML:** only `[data-node]` elements of the field's nodes are
    kept, and their attributes are checked again; everything else is
    dropped as it arrives.
  - `asDocument`:
    - reads versions 1 to 3;
    - a document holding an application node the field does not declare,
      or with invalid attributes, is read-only.
  - `toDocument` keeps application nodes with their declared attributes
    only.
  - `isEmptyDocument` counts them as content.
- **View page and read-only fields:**
  - `RichTextView` draws `<u>`, `<sup>`, `<sub>` and application nodes:
    their `view`, or the placeholder; an undeclared node shows the "unknown
    element" placeholder.
  - `RichTextEntry` never shows raw JSON. A document it cannot draw shows
    "cannot be shown", where today it shows the raw value.
  - The view page receives the field's `richNodes` in its entry schema, like
    the form.

### SDK (ui 4)

```tsx
import { registerRichTextNode } from "@rustcms/admin-sdk";

registerRichTextNode("app.kbd", {
  view: ({ attrs }) => <kbd className="rounded border px-1">{String(attrs.keys)}</kbd>,
  edit: ({ attrs, onChange, t }) => <Input value={String(attrs.keys ?? "")} onChange={(e) => onChange({ ...attrs, keys: e.target.value })} />,
});
```

- `registerRichTextNode(name, { view, edit? })` follows the rules of the
  other registrations:
  - only while extensions load;
  - one registration per name;
  - the name must be `app.*`;
  - the level is checked against `Requires.ui`.
- Props:
  - `view` gets `{ name, attrs, inline, locale, t }`;
  - `edit` gets `{ name, attrs, onChange, locale, t }`;
  - both receive copies: changing them changes nothing.
- A renderer for a node the field does not declare is never used.
- `UI_VERSION` and `SDK_UI` become 4. The SDK is copied to the CLI template
  and the demo.

### Demo

- `app.kbd`: an inline atom with a `keys` attribute and `.text`, accepted by
  the post's content, and drawn by a v2 extension's `view` and `edit`.
- `app.note_ref`: a block atom with no `view`, which shows the placeholder.

## Consequences

- Applications can put structured elements in rich text, declared in Rust
  and checked on every write. A course question's blanks, a product card or
  a shortcut no longer depend on text conventions.
- Nothing becomes HTML.
  - Application nodes are scalars with declared kinds.
  - Exported HTML is escaped, with a plain-text fallback.
  - A frontend that wants a rich rendering does it under its own CSP.
- Documents of versions 1 and 2 are untouched. A document becomes version 3
  only when it uses a version 3 part, so documents without one stay readable
  by 0.10 to 0.13.0.
- `Mark` gains variants and becomes `#[non_exhaustive]`; `Node` gains
  `App`. `InputSchema` gains `richNodes`. Applications matching these
  exhaustively must update.
- Removing a node declaration leaves the documents that use it read-only
  until they are edited without it. That is a data decision for the
  application, never made silently by the panel: the editor offers the user
  to remove the elements the field no longer accepts (amendment below).
- Left for later:
  - nodes with content;
  - media and link attributes (they would join `media::check_references` and
    `safe_href`);
  - application marks;
  - HTML rendered by the application through the custom template sanitizer
    (ADR 0019);
  - rich text in settings pages and repeater rows;
  - formulas (KaTeX).

## Amendment (0.13.1, review)

- **Bounded once completed.** Defaults are stored in every node that leaves
  them out, so the parser counts what they add to the document's size while
  it accepts each node: a document over 256 KiB once completed is refused
  (`may not be larger than 256 KiB`), whatever was sent. A node's defaults
  take at most 1 KiB together (boot). `AppNodes::complete` uses the same
  parse.
- **The server stores the lowest version.** Every rich text write (form, API,
  import, hook) stores the document in the lowest version its parts need, not
  only the editor's; an image caption or width (even empty) needs version 2.
- **Block elements stay out of callouts** at any depth (a list inside a
  callout included), on the server and in the editor
  (`has a callout holding the element `app.x``).
- **`node` is a reserved attribute name** (HTML writes `data-node` for the
  node's name, the editor reads it back).
- **Removing what a field no longer accepts.** A document holding an
  undeclared element, or one whose attributes no longer fit, opens read-only
  with a button that removes those elements and opens the rest in the editor
  (when that leaves a document the editor can hold). The server accepts the
  result as any write.
- **Labels.** `Attr::label(..)` names an attribute's input in the attribute
  form (translated like the node's label; default: its name); schemas carry
  `label` in each attribute (vocabulary 18).
- **Wire.** Integer attribute values and their `min`/`max` are JSON numbers,
  bounded to ±(2^53 − 1): an exception to ADR 0003, safe in every client.
- `Attr::uuid` is not a media reference: no visibility or reference check;
  never use it for media ids.
- Outline ids drawn by the panel can differ from `Html::outline`'s when a
  heading holds an element with text (the server includes the text).
- Actions writing rich text through the ORM are trusted writes, as for every
  field rule today (not checked again).
- A view page shows a legacy rich text value that is a string as it is.
