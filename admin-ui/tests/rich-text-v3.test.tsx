// Rich text version 3 (ADR 0042): new marks, application nodes checked
// against the field's declarations, placeholders, extension renderers.
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, configure, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { RichNodeSchema } from "@/api/types";
import { SessionContext } from "@/api/context";
import { createSession } from "@/app/session";
import type { Editor } from "@tiptap/core";
import { type DocNode, asDocument, asShownDocument, checkAttrs, isEmptyDocument, lowestVersion, toDocument, withoutUnknownElements } from "@/lib/rich-text";
import { registerRichTextNode, resetWidgets } from "@/lib/widgets";
import { RichEditorWidget, RichTextView } from "@/renderers/rich-text";

configure({ asyncUtilTimeout: 8000 });
vi.setConfig({ testTimeout: 40_000 });

const API = "http://localhost/admin/api";
const UUID = "0199D3A4-1B2C-7D3E-8F40-123456789ABC";

const KBD: RichNodeSchema = {
  name: "app.kbd",
  label: "Shortcut",
  inline: true,
  attrs: [
    { name: "keys", label: "keys", kind: "text", required: true, min: 1, max: 10 },
    { name: "size", label: "size", kind: "one_of", required: false, default: "m", options: ["s", "m"] },
  ],
};
const CARD: RichNodeSchema = {
  name: "app.card",
  label: "Card",
  inline: false,
  attrs: [
    { name: "id", label: "id", kind: "uuid", required: true },
    { name: "qty", label: "qty", kind: "integer", required: false, default: 1, min: 1, max: 9 },
    { name: "featured", label: "featured", kind: "boolean", required: false, default: false },
  ],
};
const NODES = [KBD, CARD];

const text = (t: string, ...marks: string[]): DocNode => ({ type: "text", text: t, ...(marks.length ? { marks: marks.map((type) => ({ type })) } : {}) });
const para = (...content: DocNode[]): DocNode => ({ type: "paragraph", content });
const kbd = (attrs: Record<string, unknown>): DocNode => ({ type: "app.kbd", attrs });
const doc = (version: number, ...content: DocNode[]) => ({ type: "doc", version, content });

function Providers({ children }: { children: React.ReactNode }) {
  const client = new QueryClient();
  const session = createSession({ basePath: "/admin", apiPath: API }, client, () => {});
  return (
    <SessionContext.Provider value={session}>
      <QueryClientProvider client={client}>{children}</QueryClientProvider>
    </SessionContext.Provider>
  );
}

afterEach(() => {
  cleanup();
  resetWidgets();
});

describe("documents", () => {
  it("are written in the lowest version they need", () => {
    expect(lowestVersion([para(text("x", "bold"))])).toBe(1);
    expect(lowestVersion([{ type: "callout", attrs: { tone: "info" }, content: [para(text("x"))] }])).toBe(2);
    expect(lowestVersion([para(text("x", "underline"))])).toBe(3);
    expect(lowestVersion([{ type: "callout", attrs: { tone: "info" }, content: [para(text("x", "subscript"))] }])).toBe(3);
    expect(lowestVersion([para(kbd({ keys: "A" }))])).toBe(3);
  });

  it("hold application nodes the field declares, with attributes that fit", () => {
    const ok = doc(3,
      { type: "heading", attrs: { level: 2 }, content: [text("Press "), kbd({ keys: "Ctrl+C" })] },
      para(text("H"), text("2", "subscript"), text("O"), text("n", "superscript", "underline")),
      { type: "app.card", attrs: { id: UUID } },
      { type: "bulletList", content: [{ type: "listItem", content: [{ type: "app.card", attrs: { id: UUID, qty: 9, featured: true } }] }] },
      { type: "table", content: [{ type: "tableRow", content: [{ type: "tableCell", content: [para(kbd({ keys: "A" }))] }] }] },
    );
    expect(asDocument(ok, NODES)).not.toBeNull();
    // Without the declarations, or in an older version: read-only.
    expect(asDocument(ok)).toBeNull();
    expect(asDocument({ ...ok, version: 2 }, NODES)).toBeNull();
    for (const content of [
      [para(kbd({ keys: "" }))],
      [para(kbd({ keys: "x".repeat(11) }))],
      [para(kbd({ keys: 1 }))],
      [para(kbd({}))],
      [para(kbd({ keys: "A", html: "<b>" }))],
      [para(kbd({ keys: "A", size: "xl" }))],
      [para({ ...kbd({ keys: "A" }), content: [] })],
      [para({ ...kbd({ keys: "A" }), marks: [{ type: "bold" }] })],
      [kbd({ keys: "A" })],
      [para({ type: "app.card", attrs: { id: UUID } })],
      [{ type: "app.card", attrs: { id: "nope" } }],
      [{ type: "app.card", attrs: { id: UUID, qty: 1.5 } }],
      [{ type: "app.card", attrs: { id: UUID, qty: 10 } }],
      [{ type: "callout", attrs: { tone: "info" }, content: [{ type: "app.card", attrs: { id: UUID } }] }],
      [{ type: "codeBlock", content: [kbd({ keys: "A" })] }],
      [para({ type: "app.other", attrs: {} })],
      [para(text("x", "superscript", "subscript"))],
      // `null` is not a value (the server refuses it), even with a default.
      [para(kbd({ keys: "A", size: null }))],
    ]) {
      expect(asDocument(doc(3, ...content), NODES), JSON.stringify(content)).toBeNull();
    }
    // Version 3 marks in a version 2 document.
    expect(asDocument(doc(2, para(text("x", "underline"))))).toBeNull();
    expect(isEmptyDocument(doc(3, para(kbd({ keys: "A" }))) as never)).toBe(false);
  });

  it("keep what the server accepts from the editor, defaults filled in", () => {
    const editor: DocNode = {
      type: "doc",
      content: [
        para(text("x", "superscript", "subscript"), text("y", "subscript", "superscript", "underline"), kbd({ keys: "A", size: null })),
        { type: "app.card", attrs: { id: UUID, qty: null, featured: null } },
        // Undeclared, misplaced or invalid: dropped.
        para({ type: "app.other", attrs: {} }, { type: "app.card", attrs: { id: UUID } }, kbd({ keys: "" })),
      ],
    };
    expect(toDocument(editor, API, NODES)).toEqual({
      type: "doc",
      version: 3,
      content: [
        para(text("x", "superscript"), text("y", "subscript", "underline"), kbd({ keys: "A", size: "m" })),
        { type: "app.card", attrs: { id: UUID.toLowerCase(), qty: 1, featured: false } },
        para(),
      ],
    });
    expect(checkAttrs(CARD, { id: UUID, extra: 1 })).toBeNull();
    expect(checkAttrs(CARD, { id: UUID, qty: null })).toBeNull();
  });

  it("keep block elements out of callouts, lists inside them included", () => {
    const list = { type: "bulletList", content: [{ type: "listItem", content: [para(text("i")), { type: "app.card", attrs: { id: UUID } }] }] };
    expect(asDocument(doc(3, list), NODES)).not.toBeNull();
    expect(asDocument(doc(3, { type: "callout", attrs: { tone: "info" }, content: [list] }), NODES)).toBeNull();
  });

  it("are shown only when well formed", () => {
    expect(asShownDocument(doc(3, para(text("x")), { type: "mystery", content: [para(text("y"))] }))).not.toBeNull();
    for (const content of [
      [{ type: "paragraph", content: "x" }],
      [{ type: "paragraph", content: [{ type: "text", text: 1 }] }],
      [{ type: "paragraph", content: [{ type: "text", text: "x", marks: "bold" }] }],
      [{ type: "image", attrs: ["x"] }],
      ["x"],
      [{ content: [] }],
    ]) {
      expect(asShownDocument({ type: "doc", version: 1, content }), JSON.stringify(content)).toBeNull();
    }
  });
});

describe("showing", () => {
  it("draws the new marks, extension views, and placeholders otherwise", () => {
    const View = ({ attrs }: { attrs: { keys: string } }) => <kbd data-testid="kbd">{attrs.keys}</kbd>;
    registerRichTextNode("app.kbd", { view: View as never }, { ui: 4 }, 4);
    const { container } = render(
      <Providers>
        <RichTextView
          nodes={NODES}
          doc={doc(3,
            para(text("H"), text("2", "subscript"), text("x", "superscript"), text("u", "underline"), kbd({ keys: "Ctrl+C" })),
            { type: "app.card", attrs: { id: UUID } },
            para({ type: "app.gone", attrs: { x: 1 } }, kbd({ keys: "" })),
          ) as never}
        />
      </Providers>,
    );
    expect(container.querySelector("sub")?.textContent).toBe("2");
    expect(container.querySelector("sup")?.textContent).toBe("x");
    expect(container.querySelector("u")?.textContent).toBe("u");
    expect(screen.getByTestId("kbd").textContent).toBe("Ctrl+C");
    // No view: a placeholder with its label; undeclared or invalid: "Unknown element". Never JSON.
    expect(screen.getByRole("img", { name: "Card" })).toBeTruthy();
    expect(screen.getAllByRole("img", { name: "Unknown element" })).toHaveLength(2);
    expect(container.textContent).not.toContain("{");
  });

  it("shows parts it does not know as placeholders, never dropping them silently", () => {
    render(<Providers><RichTextView doc={doc(3, { type: "mystery", content: [para(text("y"))] }, para(text("x"))) as never} /></Providers>);
    expect(screen.getByRole("img", { name: "Unknown element" })).toBeTruthy();
    expect(screen.getByText("x")).toBeTruthy();
  });

  it("shows the placeholder when a view throws", () => {
    const error = vi.spyOn(console, "error").mockImplementation(() => {});
    registerRichTextNode("app.kbd", { view: (() => { throw new Error("boom"); }) as never }, { ui: 4 }, 4);
    render(<Providers><RichTextView nodes={NODES} doc={doc(3, para(kbd({ keys: "A" }))) as never} /></Providers>);
    expect(screen.getByRole("img", { name: "Shortcut" })).toBeTruthy();
    error.mockRestore();
  });
});

describe("registerRichTextNode", () => {
  it("takes app.* names once, with a component, at the declared level", () => {
    const View = () => null;
    expect(() => registerRichTextNode("kbd", { view: View }, { ui: 4 }, 4)).toThrow(/invalid/);
    expect(() => registerRichTextNode("app.Kbd", { view: View }, { ui: 4 }, 4)).toThrow(/invalid/);
    expect(() => registerRichTextNode("app.kbd", {}, { ui: 4 }, 4)).toThrow(/view/);
    expect(() => registerRichTextNode("app.kbd", { view: "x" as never }, { ui: 4 }, 4)).toThrow(/view/);
    expect(() => registerRichTextNode("app.kbd", { view: View }, { ui: 4 }, 3)).toThrow(/ui 4/);
    registerRichTextNode("app.kbd", { view: View }, { ui: 4 }, 4);
    expect(() => registerRichTextNode("app.kbd", { edit: View }, { ui: 4 }, 4)).toThrow(/already/);
  });
});

describe("the editor", () => {
  it("inserts a declared node from the menu with its attributes, as version 3", async () => {
    let emitted = "";
    render(
      <Providers>
        <RichEditorWidget id="body" label="Body" value="" onChange={(v) => (emitted = v)} invalid={false} nullable nodes={NODES} />
      </Providers>,
    );
    const box = await screen.findByRole("textbox", { name: "Body" });
    await userEvent.click(box);
    await userEvent.click(screen.getByRole("button", { name: "Insert an element" }));
    await userEvent.click(screen.getByRole("menuitem", { name: "Shortcut" }));
    const dialog = await screen.findByRole("dialog", { name: "Shortcut" });
    // Required and empty: refused before anything is inserted.
    await userEvent.click(within(dialog).getByRole("button", { name: "Apply" }));
    expect(within(dialog).getByRole("alert").textContent).toMatch(/missing/);
    await userEvent.type(within(dialog).getByLabelText(/keys/), "Ctrl+K");
    await userEvent.click(within(dialog).getByRole("button", { name: "Apply" }));
    await waitFor(() => expect(JSON.parse(emitted)).toEqual(doc(3, para(kbd({ keys: "Ctrl+K", size: "m" })))));
    expect(within(box).getByRole("img", { name: "Shortcut" })).toBeTruthy();
  });

  it("inserts after a selected node, and edits an element where it is after other changes", async () => {
    let emitted = "";
    const value = JSON.stringify(doc(3, para(text("ab"), kbd({ keys: "A", size: "m" }))));
    render(
      <Providers>
        <RichEditorWidget id="body" label="Body" value={value} onChange={(v) => (emitted = v)} invalid={false} nullable nodes={NODES} />
      </Providers>,
    );
    const box = await screen.findByRole("textbox", { name: "Body" });
    const editor = (box as unknown as { editor: Editor }).editor;
    // The element selected: a new one goes after it, never replacing it.
    editor.commands.setNodeSelection(3);
    await userEvent.click(screen.getByRole("button", { name: "Insert an element" }));
    await userEvent.click(screen.getByRole("menuitem", { name: "Shortcut" }));
    let dialog = await screen.findByRole("dialog", { name: "Shortcut" });
    await userEvent.type(within(dialog).getByLabelText(/keys/), "B");
    await userEvent.click(within(dialog).getByRole("button", { name: "Apply" }));
    await waitFor(() => expect(JSON.parse(emitted)).toEqual(doc(3, para(text("ab"), kbd({ keys: "A", size: "m" }), kbd({ keys: "B", size: "m" })))));
    // Editing the first element while text is inserted before it: the change lands on it.
    await userEvent.dblClick(within(box).getAllByRole("img", { name: "Shortcut" })[0]!);
    dialog = await screen.findByRole("dialog", { name: "Shortcut" });
    editor.commands.insertContentAt(1, "zz");
    const keys = within(dialog).getByLabelText(/keys/);
    await userEvent.clear(keys);
    await userEvent.type(keys, "C");
    await userEvent.click(within(dialog).getByRole("button", { name: "Apply" }));
    await waitFor(() => expect(JSON.parse(emitted)).toEqual(doc(3, para(text("zzab"), kbd({ keys: "C", size: "m" }), kbd({ keys: "B", size: "m" })))));
  });

  it("opens a document with nodes the field does not declare read-only", async () => {
    const onChange = vi.fn();
    const value = JSON.stringify(doc(3, para(text("x"), { type: "app.gone", attrs: {} })));
    render(
      <Providers>
        <RichEditorWidget id="body" label="Body" value={value} onChange={onChange} invalid={false} nullable nodes={NODES} />
      </Providers>,
    );
    expect(await screen.findByText(/cannot edit/)).toBeTruthy();
    expect(screen.getByRole("img", { name: "Unknown element" })).toBeTruthy();
    expect(screen.queryByRole("textbox", { name: "Body" })).toBeNull();
    expect(onChange).not.toHaveBeenCalled();
    // The user may remove what the field no longer accepts, and edit the rest.
    await userEvent.click(screen.getByRole("button", { name: /Remove the elements/ }));
    expect(JSON.parse(onChange.mock.calls[0]![0] as string)).toEqual(doc(3, para(text("x"))));
  });

  it("offers no removal when it would not leave an editable document, or when disabled", async () => {
    const gone = JSON.stringify(doc(3, para(text("x"), { type: "app.gone", attrs: {} })));
    render(
      <Providers>
        <RichEditorWidget id="body" label="Body" value={gone} onChange={vi.fn()} invalid={false} nullable nodes={NODES} disabled />
      </Providers>,
    );
    expect(await screen.findByText(/cannot edit/)).toBeTruthy();
    expect(screen.queryByRole("button", { name: /Remove the elements/ })).toBeNull();
    // A list item holding only the element would be left empty.
    const item = doc(3, { type: "bulletList", content: [{ type: "listItem", content: [{ type: "app.gone", attrs: {} }] }] });
    expect(withoutUnknownElements(item as never, NODES)).toBeNull();
    // An element whose attributes no longer fit is removed too.
    const stale = doc(3, para(text("a"), kbd({ keys: "far too many keys" })));
    expect(withoutUnknownElements(stale as never, NODES)).toEqual(doc(3, para(text("a"))));
  });

  it.each([
    ["version 1", { content: [para(text("Hi"))], type: "doc", version: 1 }],
    ["version 3", { content: [para(text("Press", "underline"), kbd({ keys: "A", size: "m" }))], type: "doc", version: 3 }],
  ])("emits nothing when a document is only opened (%s, keys in the server's order)", async (_, stored) => {
    const onChange = vi.fn();
    const value = JSON.stringify(stored);
    render(
      <Providers>
        <RichEditorWidget id="body" label="Body" value={value} onChange={onChange} invalid={false} nullable nodes={NODES} />
      </Providers>,
    );
    await screen.findByRole("textbox", { name: "Body" });
    await new Promise((r) => setTimeout(r, 300));
    expect(onChange).not.toHaveBeenCalled();
  });
});
