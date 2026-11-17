//! Parsing: a JSON value becomes a [`Document`] only if it follows the closed
//! grammar of its version within the size limits.

use super::*;

/// Most elements (nodes, table rows and cells included).
pub const MAX_NODES: usize = 10_000;
/// Deepest nesting of blocks.
pub const MAX_DEPTH: usize = 16;
/// Most images.
pub const MAX_IMAGES: usize = 100;
/// Most embeds.
pub const MAX_EMBEDS: usize = 50;
/// Longest link address (bytes).
pub const MAX_HREF: usize = 2_048;
/// Longest image description (characters).
pub const MAX_ALT: usize = 500;
/// Longest image caption (characters).
pub const MAX_CAPTION: usize = 1_000;
/// Most rows of a table (the header row included).
pub const MAX_TABLE_ROWS: usize = 50;
/// Most columns of a table.
pub const MAX_TABLE_COLUMNS: usize = 20;

/// Marks on one text (each kind at most once, superscript and subscript
/// never together).
pub(super) const MAX_MARKS: usize = 7;

pub(super) fn err(message: impl Into<String>) -> DocumentError {
    DocumentError(message.into())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawNode {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    content: Option<Vec<RawNode>>,
    #[serde(default)]
    attrs: Option<serde_json::Map<String, Value>>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    marks: Option<Vec<RawMark>>,
    #[serde(default)]
    version: Option<u64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawMark {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    attrs: Option<serde_json::Map<String, Value>>,
}

pub(super) struct Budget<'a> {
    version: u64,
    /// Application nodes the document may hold.
    accepts: Option<Accepts<'a>>,
    app_nodes: usize,
    /// Inside a callout: no table or callout.
    in_callout: bool,
    nodes: usize,
    images: usize,
    embeds: usize,
}

impl Budget<'_> {
    /// Refuses a version 2 part (`message`) in a version 1 document.
    fn needs_v2(&self, message: &'static str) -> Result<(), DocumentError> {
        if self.version >= 2 { Ok(()) } else { Err(err(message)) }
    }

    /// Refuses a version 3 part (`message`) in an older document.
    fn needs_v3(&self, message: impl FnOnce() -> String) -> Result<(), DocumentError> {
        if self.version >= 3 { Ok(()) } else { Err(err(message())) }
    }
}

/// Parses and validates a stored or submitted document (version 1 to 3).
/// Application nodes (`app.*`) are refused: see [`AppNodes::parse`].
pub fn parse(value: &Value) -> Result<Document, DocumentError> {
    parse_with(value, None)
}

/// Parses a document that may hold the application nodes `accepts` names.
pub(crate) fn parse_with(value: &Value, accepts: Option<Accepts<'_>>) -> Result<Document, DocumentError> {
    // Bounded before any deep work.
    let bytes = serde_json::to_vec(value).map_err(|_| err("is not a document"))?.len();
    if bytes > MAX_BYTES {
        return Err(err(format!("may not be larger than {} KiB", MAX_BYTES / 1024)));
    }
    // A table nests four levels below its block (row, cell, paragraph, text).
    if depth(value, 0) > MAX_DEPTH * 2 + 4 {
        return Err(err("is nested too deeply"));
    }
    let raw: RawNode = serde_json::from_value(value.clone()).map_err(|_| err("is not a valid document"))?;
    let version = raw.version.filter(|v| (OLDEST_VERSION..=VERSION).contains(v));
    let (Some(version), "doc", None, None, None) =
        (version, raw.kind.as_str(), raw.attrs.as_ref(), raw.text.as_ref(), raw.marks.as_ref())
    else {
        return Err(err("is not a valid document (version 1 to 3)"));
    };
    let mut budget = Budget { version, accepts, app_nodes: 0, in_callout: false, nodes: 0, images: 0, embeds: 0 };
    let content = raw.content.unwrap_or_default();
    // List items only inside lists (as the editor's schema requires).
    if content.iter().any(|n| n.kind == "listItem") {
        return Err(err("has a list item outside a list"));
    }
    let content = blocks(content, &mut budget, 1)?;
    Ok(Document { version, content })
}

/// JSON nesting depth (arrays and objects).
pub(super) fn depth(value: &Value, level: usize) -> usize {
    match value {
        Value::Array(items) => items.iter().map(|v| depth(v, level + 1)).max().unwrap_or(level + 1),
        Value::Object(map) => map.values().map(|v| depth(v, level + 1)).max().unwrap_or(level + 1),
        _ => level,
    }
}

pub(super) fn count(budget: &mut Budget<'_>, level: usize) -> Result<(), DocumentError> {
    budget.nodes += 1;
    if budget.nodes > MAX_NODES {
        return Err(err(format!("may not have more than {MAX_NODES} elements")));
    }
    if level > MAX_DEPTH {
        return Err(err("is nested too deeply"));
    }
    Ok(())
}

pub(super) fn no_extras(
    raw: &RawNode,
    attrs: bool,
    text: bool,
    marks: bool,
    content: bool,
) -> Result<(), DocumentError> {
    let ok = (attrs || raw.attrs.is_none())
        && (text || raw.text.is_none())
        && (marks || raw.marks.is_none())
        && (content || raw.content.is_none())
        && raw.version.is_none();
    if ok { Ok(()) } else { Err(err(format!("has unexpected properties on `{}`", raw.kind.escape_debug()))) }
}

/// Only the attributes `allowed` (else `message`).
fn only_attrs(
    attrs: &serde_json::Map<String, Value>,
    allowed: &[&str],
    message: &'static str,
) -> Result<(), DocumentError> {
    if attrs.keys().all(|k| allowed.contains(&k.as_str())) { Ok(()) } else { Err(err(message)) }
}

pub(super) fn blocks(raw: Vec<RawNode>, budget: &mut Budget<'_>, level: usize) -> Result<Vec<Node>, DocumentError> {
    raw.into_iter().map(|node| block(node, budget, level)).collect()
}

/// At least one paragraph, nothing else (table cells; else `message`).
fn paragraphs(
    raw: Option<Vec<RawNode>>,
    budget: &mut Budget<'_>,
    level: usize,
    message: &'static str,
) -> Result<Vec<Node>, DocumentError> {
    let items = raw.unwrap_or_default();
    if items.is_empty() || items.iter().any(|n| n.kind != "paragraph") {
        return Err(err(message));
    }
    blocks(items, budget, level)
}

pub(super) fn block(raw: RawNode, budget: &mut Budget<'_>, level: usize) -> Result<Node, DocumentError> {
    count(budget, level)?;
    if budget.in_callout && (raw.kind == "table" || raw.kind == "callout") {
        return Err(err("has a callout holding a table or a callout"));
    }
    Ok(match raw.kind.as_str() {
        "paragraph" => {
            no_extras(&raw, false, false, false, true)?;
            Node::Paragraph(inlines(raw.content.unwrap_or_default(), budget, level + 1)?)
        }
        "heading" => {
            no_extras(&raw, true, false, false, true)?;
            let attrs = raw.attrs.clone().unwrap_or_default();
            let level_attr = attrs.get("level").and_then(Value::as_u64).filter(|l| (2..=4).contains(l));
            let Some(heading) = level_attr.filter(|_| attrs.len() == 1) else {
                return Err(err("has a heading without a level from 2 to 4"));
            };
            Node::Heading(heading as u8, inlines(raw.content.unwrap_or_default(), budget, level + 1)?)
        }
        "bulletList" | "orderedList" => {
            no_extras(&raw, false, false, false, true)?;
            let kind = raw.kind.clone();
            let items = raw.content.unwrap_or_default();
            if items.iter().any(|i| i.kind != "listItem") {
                return Err(err("has a list holding something else than list items"));
            }
            let items = blocks(items, budget, level + 1)?;
            if kind == "bulletList" { Node::BulletList(items) } else { Node::OrderedList(items) }
        }
        "listItem" | "blockquote" => {
            no_extras(&raw, false, false, false, true)?;
            let kind = raw.kind.clone();
            let items = raw.content.unwrap_or_default();
            if items.iter().any(|n| n.kind == "listItem") {
                return Err(err("has a list item outside a list"));
            }
            let items = blocks(items, budget, level + 1)?;
            if kind == "listItem" { Node::ListItem(items) } else { Node::Blockquote(items) }
        }
        "codeBlock" => {
            no_extras(&raw, false, false, false, true)?;
            let mut text = String::new();
            for part in raw.content.unwrap_or_default() {
                match part {
                    RawNode { kind, text: Some(t), marks: None, attrs: None, content: None, version: None }
                        if kind == "text" =>
                    {
                        count(budget, level + 1)?;
                        text.push_str(&t);
                    }
                    _ => return Err(err("has a code block holding something else than plain text")),
                }
            }
            Node::CodeBlock(text)
        }
        "horizontalRule" => {
            no_extras(&raw, false, false, false, false)?;
            Node::HorizontalRule
        }
        "image" => image(raw, budget)?,
        "table" => table(raw, budget, level)?,
        "embed" => embed(raw, budget)?,
        "callout" => {
            budget.needs_v2("has a callout, which needs document version 2")?;
            no_extras(&raw, true, false, false, true)?;
            let attrs = raw.attrs.unwrap_or_default();
            only_attrs(&attrs, &["tone"], "has a callout with unexpected attributes")?;
            let tone = match attrs.get("tone").and_then(Value::as_str) {
                Some("info") => CalloutTone::Info,
                Some("warn") => CalloutTone::Warn,
                _ => return Err(err("has a callout whose tone is not `info` or `warn`")),
            };
            let items = raw.content.unwrap_or_default();
            if items.is_empty()
                || items.iter().any(|n| !matches!(n.kind.as_str(), "paragraph" | "bulletList" | "orderedList"))
            {
                return Err(err("has a callout holding something else than paragraphs and lists"));
            }
            budget.in_callout = true;
            let items = blocks(items, budget, level + 1);
            budget.in_callout = false;
            Node::Callout(tone, items?)
        }
        other if other.starts_with("app.") => app_node(raw, budget, false)?,
        other => return Err(err(format!("has an unknown block `{}`", other.escape_debug()))),
    })
}

/// An application node, where a block (`inline: false`) or an inline goes.
pub(super) fn app_node(raw: RawNode, budget: &mut Budget<'_>, inline: bool) -> Result<Node, DocumentError> {
    let name = raw.kind.escape_debug().to_string();
    let Some(decl) = budget.accepts.and_then(|a| a.get(&raw.kind)) else {
        return Err(err(format!("has an unknown element `{name}`")));
    };
    budget.needs_v3(|| format!("has the element `{name}`, which needs document version 3"))?;
    if decl.is_inline() != inline {
        return Err(err(format!("has an element `{name}` out of place")));
    }
    no_extras(&raw, true, false, false, false)?;
    budget.app_nodes += 1;
    if budget.app_nodes > MAX_APP_NODES {
        return Err(err(format!("may not have more than {MAX_APP_NODES} application elements")));
    }
    let attrs = decl.accept(&raw.attrs.unwrap_or_default())?;
    Ok(Node::App(AppNode { name: raw.kind, inline, attrs }))
}

pub(super) fn image(raw: RawNode, budget: &mut Budget<'_>) -> Result<Node, DocumentError> {
    no_extras(&raw, true, false, false, false)?;
    budget.images += 1;
    if budget.images > MAX_IMAGES {
        return Err(err(format!("may not have more than {MAX_IMAGES} images")));
    }
    let attrs = raw.attrs.unwrap_or_default();
    only_attrs(&attrs, &["media", "alt", "caption", "width"], "has an image with unexpected attributes")?;
    let media = attrs
        .get("media")
        .and_then(Value::as_str)
        .and_then(|id| uuid::Uuid::parse_str(id).ok())
        .ok_or_else(|| err("has an image that is not a media item"))?;
    let alt = match attrs.get("alt") {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(alt)) if alt.chars().count() <= MAX_ALT => alt.clone(),
        _ => return Err(err(format!("has an image description longer than {MAX_ALT} characters"))),
    };
    let caption = match attrs.get("caption") {
        None => String::new(),
        Some(_) if budget.version < 2 => return Err(err("has an image caption, which needs document version 2")),
        Some(Value::Null) => String::new(),
        Some(Value::String(caption)) if caption.chars().count() <= MAX_CAPTION => caption.clone(),
        _ => return Err(err(format!("has an image caption longer than {MAX_CAPTION} characters"))),
    };
    let width = match attrs.get("width") {
        None => ImageWidth::Normal,
        Some(_) if budget.version < 2 => return Err(err("has an image width, which needs document version 2")),
        Some(Value::String(w)) if w == "normal" => ImageWidth::Normal,
        Some(Value::String(w)) if w == "wide" => ImageWidth::Wide,
        Some(Value::String(w)) if w == "full" => ImageWidth::Full,
        _ => return Err(err("has an image width other than `normal`, `wide` or `full`")),
    };
    Ok(Node::Image(Image { media, alt, caption, width }))
}

pub(super) fn table(raw: RawNode, budget: &mut Budget<'_>, level: usize) -> Result<Node, DocumentError> {
    budget.needs_v2("has a table, which needs document version 2")?;
    no_extras(&raw, false, false, false, true)?;
    let rows = raw.content.unwrap_or_default();
    if rows.is_empty() || rows.len() > MAX_TABLE_ROWS {
        return Err(err(format!("has a table with no rows or more than {MAX_TABLE_ROWS} rows")));
    }
    let header = rows[0].content.as_ref().and_then(|cells| cells.first()).is_some_and(|c| c.kind == "tableHeader");
    let mut width = None;
    let mut out = Vec::with_capacity(rows.len());
    for (index, row) in rows.into_iter().enumerate() {
        count(budget, level + 1)?;
        if row.kind != "tableRow" {
            return Err(err("has a table holding something else than rows"));
        }
        no_extras(&row, false, false, false, true)?;
        let cells = row.content.unwrap_or_default();
        if cells.is_empty() || cells.len() > MAX_TABLE_COLUMNS {
            return Err(err(format!("has a table row with no cells or more than {MAX_TABLE_COLUMNS} cells")));
        }
        if *width.get_or_insert(cells.len()) != cells.len() {
            return Err(err("has a table whose rows have different lengths"));
        }
        // Header cells make up the first row, or nothing.
        let kind = if header && index == 0 { "tableHeader" } else { "tableCell" };
        let mut parsed = Vec::with_capacity(cells.len());
        for cell in cells {
            count(budget, level + 2)?;
            if cell.kind != kind {
                return Err(err("has a table cell out of place (header cells make up the first row only)"));
            }
            no_extras(&cell, false, false, false, true)?;
            let content =
                paragraphs(cell.content, budget, level + 3, "has a table cell holding something else than paragraphs")?;
            parsed.push(Cell { content });
        }
        out.push(parsed);
    }
    Ok(Node::Table(Table { header, rows: out }))
}

pub(super) fn embed(raw: RawNode, budget: &mut Budget<'_>) -> Result<Node, DocumentError> {
    budget.needs_v2("has an embed, which needs document version 2")?;
    no_extras(&raw, true, false, false, false)?;
    budget.embeds += 1;
    if budget.embeds > MAX_EMBEDS {
        return Err(err(format!("may not have more than {MAX_EMBEDS} embeds")));
    }
    let attrs = raw.attrs.unwrap_or_default();
    only_attrs(&attrs, &["provider", "id"], "has an embed with unexpected attributes")?;
    let provider = attrs
        .get("provider")
        .and_then(Value::as_str)
        .and_then(EmbedProvider::from_name)
        .ok_or_else(|| err("has an embed whose provider is not youtube or vimeo"))?;
    let id = attrs.get("id").and_then(Value::as_str).unwrap_or_default();
    if !provider.valid_id(id) {
        return Err(err("has an embed with an invalid id"));
    }
    Ok(Node::Embed(Embed { provider, id: id.to_owned() }))
}

pub(super) fn inlines(raw: Vec<RawNode>, budget: &mut Budget<'_>, level: usize) -> Result<Vec<Node>, DocumentError> {
    let mut out = Vec::with_capacity(raw.len());
    for node in raw {
        count(budget, level)?;
        out.push(match node.kind.as_str() {
            "text" => {
                no_extras(&node, false, true, true, false)?;
                let text = node.text.ok_or_else(|| err("has a text without text"))?;
                if text.is_empty() {
                    return Err(err("has an empty text"));
                }
                let raw_marks = node.marks.unwrap_or_default();
                if raw_marks.len() > MAX_MARKS {
                    return Err(err("has a text with too many marks"));
                }
                let marks: Vec<Mark> = raw_marks.into_iter().map(|m| mark(m, budget)).collect::<Result<_, _>>()?;
                let kinds: std::collections::HashSet<_> = marks.iter().map(std::mem::discriminant).collect();
                if kinds.len() != marks.len() {
                    return Err(err("has a text with the same mark twice"));
                }
                if marks.contains(&Mark::Superscript) && marks.contains(&Mark::Subscript) {
                    return Err(err("has a text both superscript and subscript"));
                }
                Node::Text(text, marks)
            }
            "hardBreak" => {
                no_extras(&node, false, false, false, false)?;
                Node::HardBreak
            }
            other if other.starts_with("app.") => app_node(node, budget, true)?,
            // Images are blocks (the editor's schema): never inside text.
            other => return Err(err(format!("has an unknown inline `{}`", other.escape_debug()))),
        });
    }
    Ok(out)
}

pub(super) fn mark(raw: RawMark, budget: &Budget<'_>) -> Result<Mark, DocumentError> {
    let plain = |mark: Mark| if raw.attrs.is_none() { Ok(mark) } else { Err(err("has a mark with attributes")) };
    let v3 = |mark: Mark| {
        budget.needs_v3(|| format!("has the mark `{}`, which needs document version 3", raw.kind))?;
        plain(mark)
    };
    match raw.kind.as_str() {
        "bold" => plain(Mark::Bold),
        "italic" => plain(Mark::Italic),
        "strike" => plain(Mark::Strike),
        "code" => plain(Mark::Code),
        "underline" => v3(Mark::Underline),
        "superscript" => v3(Mark::Superscript),
        "subscript" => v3(Mark::Subscript),
        "link" => {
            let attrs = raw.attrs.unwrap_or_default();
            if attrs.keys().any(|k| k != "href") {
                return Err(err("has a link with unexpected attributes"));
            }
            let href = attrs.get("href").and_then(Value::as_str).unwrap_or_default();
            if safe_href(href) {
                Ok(Mark::Link(href.to_owned()))
            } else {
                Err(err("has a link that is not http, https or mailto"))
            }
        }
        other => Err(err(format!("has an unknown mark `{}`", other.escape_debug()))),
    }
}
