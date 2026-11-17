// Rich text version 3 (ADR 0042) in the editor and on view pages: the marks
// superscript and subscript, and the application's nodes. The panel builds a
// TipTap atom for each node a field accepts (`richNodes`); an extension only
// gives React components (`registerRichTextNode`): `view` shows the node,
// `edit` changes its attributes. Without them: a labelled placeholder, and a
// form made from the attributes' kinds. Nothing renders a `style` attribute.
import { Mark, Node, mergeAttributes } from "@tiptap/core";
import { ReactNodeViewRenderer, type ReactNodeViewProps, useReactNodeView } from "@tiptap/react";
import { Component, type ComponentType, type ReactNode, useId, useMemo, useState } from "react";
import type { RichAttrSchema, RichNodeSchema } from "@/api/types";
import { Button } from "@/components/ui/button";
import { Switch } from "@/components/ui/checkbox";
import { Input, Label, NativeSelect } from "@/components/ui/input";
import { useLocale, useT } from "@/i18n";
import { attrFits, checkAttrs, withoutUnset } from "@/lib/rich-text";
import { richTextNode, translateExtension } from "@/lib/widgets";

declare module "@tiptap/core" {
  interface Commands<ReturnType> {
    superscript: {
      /** Toggles superscript (removing subscript). */
      toggleSuperscript: () => ReturnType;
    };
    subscript: {
      /** Toggles subscript (removing superscript). */
      toggleSubscript: () => ReturnType;
    };
  }
}

/** Superscript (version 3): never with subscript on the same text. */
export const RichSuperscript = Mark.create({
  name: "superscript",
  excludes: "subscript",
  parseHTML: () => [{ tag: "sup" }],
  renderHTML: ({ HTMLAttributes }) => ["sup", mergeAttributes(HTMLAttributes), 0],
  addCommands() {
    return { toggleSuperscript: () => ({ commands }) => commands.toggleMark(this.name) };
  },
  addKeyboardShortcuts() {
    return { "Mod-.": () => this.editor.commands.toggleSuperscript() };
  },
});

/** Subscript (version 3): never with superscript on the same text. */
export const RichSubscript = Mark.create({
  name: "subscript",
  excludes: "superscript",
  parseHTML: () => [{ tag: "sub" }],
  renderHTML: ({ HTMLAttributes }) => ["sub", mergeAttributes(HTMLAttributes), 0],
  addCommands() {
    return { toggleSubscript: () => ({ commands }) => commands.toggleMark(this.name) };
  },
  addKeyboardShortcuts() {
    return { "Mod-,": () => this.editor.commands.toggleSubscript() };
  },
});

/** A `data-*` attribute's text as the attribute's value, or undefined. */
function fromData(attr: RichAttrSchema, text: string | null): unknown {
  if (text === null) return undefined;
  switch (attr.kind) {
    case "integer":
      return /^-?\d+$/.test(text) ? Number(text) : undefined;
    case "boolean":
      return text === "true" ? true : text === "false" ? false : undefined;
    default:
      return text;
  }
}

/**
 * The TipTap nodes of a field's application nodes: atoms, drawn by React;
 * `onEdit(position)` opens the attribute dialog for the node at `position`.
 * Pasted HTML keeps only `[data-node]` elements of these nodes whose
 * attributes fit (anything else is dropped as it arrives).
 */
export function appNodeExtensions(nodes: readonly RichNodeSchema[], onEdit: (position: number) => void) {
  return nodes.map((decl) => {
    const tag = decl.inline ? "span" : "div";
    return Node.create({
      name: decl.name,
      group: decl.inline ? "inline" : "block",
      inline: decl.inline,
      atom: true,
      selectable: true,
      draggable: true,
      addAttributes() {
        return Object.fromEntries(decl.attrs.map((attr) => [attr.name, { default: attr.default ?? null, rendered: false }]));
      },
      parseHTML() {
        return [{
          tag: `${tag}[data-node]`,
          getAttrs: (el) => {
            if (el.getAttribute("data-node") !== decl.name) return false;
            const given: Record<string, unknown> = {};
            for (const attr of decl.attrs) {
              const value = fromData(attr, el.getAttribute(`data-${attr.name}`));
              if (value !== undefined) given[attr.name] = value;
            }
            return checkAttrs(decl, given) ?? false;
          },
        }];
      },
      renderHTML({ node }) {
        const data: Record<string, string> = { "data-node": decl.name };
        for (const attr of decl.attrs) {
          const value = node.attrs[attr.name];
          if (value !== null && value !== undefined) data[`data-${attr.name}`] = String(value);
        }
        return [tag, data];
      },
      addNodeView() {
        return ReactNodeViewRenderer((props: ReactNodeViewProps) => (
          <Wrapper
            tag={tag}
            className={decl.inline ? "rich-app-node" : "rich-app-node rich-app-block"}
            name={decl.name}
            onDoubleClick={() => {
              const position = props.getPos();
              if (typeof position === "number" && props.editor.isEditable) onEdit(position);
            }}
          >
            <AppNode decl={decl} attrs={props.node.attrs} selected={props.selected} />
          </Wrapper>
        ));
      },
    });
  });
}

/**
 * TipTap's `NodeViewWrapper` without its `style` attribute (the panel's
 * nodes render none): the wrapper ProseMirror drags.
 */
function Wrapper({ tag, className, name, onDoubleClick, children }: {
  tag: "span" | "div";
  className: string;
  name: string;
  onDoubleClick: () => void;
  children: ReactNode;
}) {
  const { onDragStart } = useReactNodeView();
  const Tag = tag;
  return (
    <Tag data-node-view-wrapper="" data-node={name} className={className} onDragStart={(e) => onDragStart?.(e.nativeEvent)} onDoubleClick={onDoubleClick}>
      {children}
    </Tag>
  );
}

/** A copy of `value` nobody can change. */
function frozen<T>(value: T): T {
  const copy = structuredClone(value);
  const freeze = (v: unknown) => {
    if (v && typeof v === "object") {
      Object.freeze(v);
      Object.values(v).forEach(freeze);
    }
  };
  freeze(copy);
  return copy;
}

/**
 * An application node as shown: the extension's `view` (behind a boundary),
 * else a placeholder with its label. `decl` absent: a node the field does not
 * declare (an "unknown element").
 */
export function AppNode({ decl, name, attrs, selected = false }: {
  decl?: RichNodeSchema;
  name?: string;
  attrs: Record<string, unknown> | undefined;
  selected?: boolean;
}) {
  const t = useT();
  const locale = useLocale();
  const nodeName = decl?.name ?? name ?? "";
  const View = decl ? (richTextNode(decl.name)?.view as ComponentType<Record<string, unknown>> | undefined) : undefined;
  // Keyed by the attributes' text: a new object with the same values is not copied again.
  const key = JSON.stringify(attrs ?? {});
  const props = useMemo(
    () => ({ name: nodeName, attrs: frozen(JSON.parse(key) as Record<string, unknown>), inline: decl?.inline ?? true, locale, t: translateExtension }),
    [nodeName, key, decl, locale],
  );
  const placeholder = (
    <span className="rich-app-placeholder" data-selected={selected || undefined} role="img" aria-label={decl ? decl.label : t("rich.unknownElement")}>
      {decl ? decl.label : t("rich.unknownElement")}
    </span>
  );
  if (!View) return placeholder;
  return (
    <NodeBoundary name={nodeName} fallback={placeholder}>
      <span className="rich-app-view" data-selected={selected || undefined}>
        <View {...props} />
      </span>
    </NodeBoundary>
  );
}

/** A failing `view` or `edit` shows its fallback (logged), never breaks the page. */
class NodeBoundary extends Component<{ name: string; fallback: ReactNode; children: ReactNode }, { failed: boolean }> {
  state = { failed: false };

  static getDerivedStateFromError() {
    return { failed: true };
  }

  componentDidCatch(error: unknown) {
    console.error(`rustcms: rich text node ${this.props.name} failed`, error);
  }

  render() {
    return this.state.failed ? this.props.fallback : this.props.children;
  }
}

/**
 * The attributes of an application node, edited: the extension's `edit`
 * component, else a form made from their kinds. Applied only when they fit
 * the declaration (the server checks them again).
 */
export function AppNodeFields({ decl, initial, onClose, onApply }: {
  decl: RichNodeSchema;
  initial: Record<string, unknown>;
  onClose: () => void;
  onApply: (attrs: Record<string, unknown>) => void;
}) {
  const t = useT();
  const locale = useLocale();
  const [attrs, setAttrs] = useState<Record<string, unknown>>(() => ({ ...initial }));
  const [invalid, setInvalid] = useState(false);
  const Edit = richTextNode(decl.name)?.edit as ComponentType<Record<string, unknown>> | undefined;
  // What the extension's `edit` gets: a copy, made again only when they change.
  const shown = useMemo(() => frozen(attrs), [attrs]);
  const apply = () => {
    // An emptied input takes the default (or is missing, when required).
    const checked = checkAttrs(decl, withoutUnset(attrs));
    if (!checked) return setInvalid(true);
    onApply(checked);
  };
  const change = (next: unknown) => {
    if (next && typeof next === "object" && !Array.isArray(next)) {
      setAttrs({ ...(next as Record<string, unknown>) });
      setInvalid(false);
    }
  };
  const generated = <GeneratedFields decl={decl} attrs={attrs} onChange={change} onEnter={apply} />;
  return (
    <div className="flex flex-col gap-4">
      {Edit ? (
        <NodeBoundary name={decl.name} fallback={generated}>
          <Edit name={decl.name} attrs={shown} onChange={change} locale={locale} t={translateExtension} />
        </NodeBoundary>
      ) : generated}
      {invalid && <p role="alert" className="text-xs leading-5 text-destructive">{t("rich.elementInvalid")}</p>}
      <div className="flex flex-wrap justify-end gap-2">
        <Button type="button" variant="outline" onClick={onClose}>{t("action.cancel")}</Button>
        <Button type="button" onClick={apply}>{t("rich.linkApply")}</Button>
      </div>
    </div>
  );
}

/** One input per attribute, by kind. */
function GeneratedFields({ decl, attrs, onChange, onEnter }: {
  decl: RichNodeSchema;
  attrs: Record<string, unknown>;
  onChange: (attrs: Record<string, unknown>) => void;
  onEnter: () => void;
}) {
  const t = useT();
  const id = useId();
  const set = (name: string, value: unknown) => onChange({ ...attrs, [name]: value });
  const keys = (e: React.KeyboardEvent) => {
    if (e.nativeEvent.isComposing || e.nativeEvent.keyCode === 229) return;
    if (e.key === "Enter") {
      e.preventDefault();
      onEnter();
    }
  };
  if (decl.attrs.length === 0) return <p className="text-sm text-muted-foreground">{t("rich.elementNoAttributes")}</p>;
  return (
    <>
      {decl.attrs.map((attr, i) => {
        const field = `${id}-${i}`;
        const value = attrs[attr.name] ?? attr.default ?? null;
        const wrong = value !== null && !attrFits(attr, value);
        const label = <Label htmlFor={field}>{attr.name}{attr.required ? " *" : ""}</Label>;
        switch (attr.kind) {
          case "boolean":
            return (
              <div key={attr.name} className="flex items-center gap-3">
                <Switch id={field} checked={value === true} onCheckedChange={(on) => set(attr.name, on)} />
                {label}
              </div>
            );
          case "one_of":
            return (
              <div key={attr.name} className="flex flex-col gap-2">
                {label}
                <NativeSelect id={field} value={typeof value === "string" ? value : ""} onChange={(e) => set(attr.name, e.target.value || null)}>
                  {!attr.required && attr.default === undefined && <option value="">—</option>}
                  {attr.required && value === null && <option value="">—</option>}
                  {(attr.options ?? []).map((o) => <option key={o} value={o}>{o}</option>)}
                </NativeSelect>
              </div>
            );
          case "integer":
            return (
              <div key={attr.name} className="flex flex-col gap-2">
                {label}
                <Input
                  id={field}
                  type="number"
                  step={1}
                  min={attr.min}
                  max={attr.max}
                  value={typeof value === "number" ? String(value) : ""}
                  aria-invalid={wrong || undefined}
                  onChange={(e) => set(attr.name, e.target.value === "" ? null : Number(e.target.value))}
                  onKeyDown={keys}
                />
              </div>
            );
          default:
            return (
              <div key={attr.name} className="flex flex-col gap-2">
                {label}
                <Input
                  id={field}
                  value={typeof value === "string" ? value : ""}
                  aria-invalid={wrong || undefined}
                  autoFocus={i === 0}
                  onChange={(e) => set(attr.name, e.target.value)}
                  onKeyDown={keys}
                />
              </div>
            );
        }
      })}
    </>
  );
}
