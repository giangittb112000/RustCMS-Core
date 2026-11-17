//! Application nodes (document version 3): atoms an application declares,
//! with typed attributes, checked like the rest of the document. Never HTML:
//! attributes are JSON scalars of a declared kind.

use std::collections::HashMap;
use std::ops::RangeInclusive;
use std::sync::Arc;

use super::*;

/// Most application nodes in one document.
pub const MAX_APP_NODES: usize = 500;
/// Most attributes of one application node.
pub const MAX_NODE_ATTRS: usize = 16;
/// Default longest text attribute (characters).
pub const DEFAULT_TEXT_MAX: usize = 1_000;
/// Longest text attribute a declaration may allow (characters).
pub const MAX_TEXT_ATTR: usize = 10_000;
/// Most options of a `one_of` attribute.
pub const MAX_ATTR_OPTIONS: usize = 50;
/// Bytes the defaults of one node's attributes may add to a document, together.
pub const MAX_NODE_DEFAULTS: usize = 1024;
/// Largest integer attribute (exact in JavaScript): ±(2^53 − 1).
pub const MAX_SAFE_INTEGER: i64 = (1 << 53) - 1;

/// The kind of an attribute.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum AttrKind {
    /// A string of `min..=max` characters.
    Text {
        /// Fewest characters.
        min: usize,
        /// Most characters.
        max: usize,
    },
    /// An integer in `min..=max`.
    Integer {
        /// Smallest value.
        min: i64,
        /// Largest value.
        max: i64,
    },
    /// `true` or `false`.
    Boolean,
    /// One string of a closed set.
    OneOf(Vec<String>),
    /// A UUID, stored in lower case with hyphens.
    Uuid,
}

/// An attribute of an application node: its kind, and a default (else it is
/// required).
#[derive(Debug, Clone, PartialEq)]
pub struct Attr {
    kind: AttrKind,
    default: Option<Value>,
    label: Option<String>,
}

impl Attr {
    /// A string, at most 1,000 characters (see [`Attr::max`]).
    pub fn text() -> Self {
        Self { kind: AttrKind::Text { min: 0, max: DEFAULT_TEXT_MAX }, default: None, label: None }
    }

    /// An integer, any safe value (see [`Attr::range`]).
    pub fn integer() -> Self {
        Self { kind: AttrKind::Integer { min: -MAX_SAFE_INTEGER, max: MAX_SAFE_INTEGER }, default: None, label: None }
    }

    /// `true` or `false`.
    pub fn boolean() -> Self {
        Self { kind: AttrKind::Boolean, default: None, label: None }
    }

    /// One of `options` (at most 50).
    pub fn one_of<S: Into<String>>(options: impl IntoIterator<Item = S>) -> Self {
        Self { kind: AttrKind::OneOf(options.into_iter().map(Into::into).collect()), default: None, label: None }
    }

    /// A UUID.
    pub fn uuid() -> Self {
        Self { kind: AttrKind::Uuid, default: None, label: None }
    }

    /// Most characters of a text (at most 10,000).
    ///
    /// # Panics
    /// On another kind.
    pub fn max(mut self, max: usize) -> Self {
        match &mut self.kind {
            AttrKind::Text { max: m, .. } => *m = max,
            _ => panic!("rich text attribute: `max` applies to `Attr::text()` only"),
        }
        self
    }

    /// Fewest characters of a text.
    ///
    /// # Panics
    /// On another kind.
    pub fn min(mut self, min: usize) -> Self {
        match &mut self.kind {
            AttrKind::Text { min: m, .. } => *m = min,
            _ => panic!("rich text attribute: `min` applies to `Attr::text()` only"),
        }
        self
    }

    /// The range of an integer.
    ///
    /// # Panics
    /// On another kind.
    pub fn range(mut self, range: RangeInclusive<i64>) -> Self {
        match &mut self.kind {
            AttrKind::Integer { min, max } => (*min, *max) = (*range.start(), *range.end()),
            _ => panic!("rich text attribute: `range` applies to `Attr::integer()` only"),
        }
        self
    }

    /// The value of the attribute when a document leaves it out (filled in
    /// before the document is stored). Without one, it is required.
    pub fn default(mut self, value: impl Into<Value>) -> Self {
        self.default = Some(value.into());
        self
    }

    /// Its kind.
    pub fn kind(&self) -> &AttrKind {
        &self.kind
    }

    /// Its default, if any.
    pub fn default_value(&self) -> Option<&Value> {
        self.default.as_ref()
    }

    /// The label of its input in the panel's attribute form (translated like
    /// the node's label; default: the attribute's name).
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Its label, if declared.
    pub fn label_text(&self) -> Option<&str> {
        self.label.as_deref()
    }

    /// `value` as stored (a UUID in lower case), or `None` if it does not fit.
    pub(super) fn accept(&self, value: &Value) -> Option<Value> {
        match (&self.kind, value) {
            (AttrKind::Text { min, max }, Value::String(s)) => {
                let n = s.chars().count();
                (n >= *min && n <= *max).then(|| value.clone())
            }
            (AttrKind::Integer { min, max }, Value::Number(n)) => {
                n.as_i64().filter(|v| v >= min && v <= max).map(Value::from)
            }
            (AttrKind::Boolean, Value::Bool(_)) => Some(value.clone()),
            (AttrKind::OneOf(options), Value::String(s)) => options.contains(s).then(|| value.clone()),
            (AttrKind::Uuid, Value::String(s)) => {
                uuid::Uuid::parse_str(s).ok().filter(|_| s.len() == 36).map(|id| Value::from(id.to_string()))
            }
            _ => None,
        }
    }
}

/// The attributes of an application node in a document (checked).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NodeAttrs(serde_json::Map<String, Value>);

impl NodeAttrs {
    /// The value of `name`, if any.
    pub fn get(&self, name: &str) -> Option<&Value> {
        self.0.get(name)
    }

    /// A text attribute (empty when absent or of another kind).
    pub fn text(&self, name: &str) -> &str {
        self.0.get(name).and_then(Value::as_str).unwrap_or_default()
    }

    /// An integer attribute.
    pub fn integer(&self, name: &str) -> Option<i64> {
        self.0.get(name).and_then(Value::as_i64)
    }

    /// A boolean attribute (`false` when absent).
    pub fn boolean(&self, name: &str) -> bool {
        self.0.get(name).and_then(Value::as_bool).unwrap_or(false)
    }

    /// Every attribute, by name.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &Value)> {
        self.0.iter().map(|(k, v)| (k.as_str(), v))
    }

    /// As a JSON object.
    pub fn as_map(&self) -> &serde_json::Map<String, Value> {
        &self.0
    }
}

/// An application node in a document.
#[derive(Debug, Clone, PartialEq)]
pub struct AppNode {
    /// Its name (`app.…`).
    pub name: String,
    /// Inline (inside text) or a block.
    pub inline: bool,
    /// Its attributes, defaults filled in.
    pub attrs: NodeAttrs,
}

type TextFn = Arc<dyn Fn(&NodeAttrs) -> String + Send + Sync>;

/// A node an application declares: an atom (no content), inline or a block,
/// with typed attributes.
///
/// ```
/// use rustcms_admin::rich_text::{Attr, RichTextNode};
///
/// let kbd = RichTextNode::inline_atom("app.kbd")
///     .label("Keyboard shortcut")
///     .attr("keys", Attr::text().min(1).max(100))
///     .text(|attrs| attrs.text("keys").to_owned());
/// ```
#[derive(Clone)]
pub struct RichTextNode {
    name: String,
    inline: bool,
    label: Option<String>,
    attrs: Vec<(String, Attr)>,
    text: Option<TextFn>,
}

impl std::fmt::Debug for RichTextNode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RichTextNode")
            .field("name", &self.name)
            .field("inline", &self.inline)
            .field("label", &self.label)
            .field("attrs", &self.attrs)
            .field("text", &self.text.is_some())
            .finish()
    }
}

impl RichTextNode {
    /// A node inside text (`app.[a-z][a-z0-9_]{0,31}`).
    pub fn inline_atom(name: impl Into<String>) -> Self {
        Self { name: name.into(), inline: true, label: None, attrs: Vec::new(), text: None }
    }

    /// A block (`app.[a-z][a-z0-9_]{0,31}`).
    pub fn block_atom(name: impl Into<String>) -> Self {
        Self { inline: false, ..Self::inline_atom(name) }
    }

    /// Its name in the panel (the Insert menu, placeholders); translated
    /// like input labels. Default: its name.
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Adds an attribute (`[a-z][a-z0-9_]{0,31}`, at most 16).
    pub fn attr(mut self, name: impl Into<String>, attr: Attr) -> Self {
        self.attrs.push((name.into(), attr));
        self
    }

    /// Its plain text, for exported HTML and outlines (default: none). Called
    /// while rendering: keep it fast and pure.
    pub fn text(mut self, f: impl Fn(&NodeAttrs) -> String + Send + Sync + 'static) -> Self {
        self.text = Some(Arc::new(f));
        self
    }

    /// Its name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Whether it is inline.
    pub fn is_inline(&self) -> bool {
        self.inline
    }

    /// Its label (its name when none was given).
    pub fn label_text(&self) -> &str {
        self.label.as_deref().unwrap_or(&self.name)
    }

    /// Its attributes, in declaration order.
    pub fn attrs(&self) -> impl Iterator<Item = (&str, &Attr)> {
        self.attrs.iter().map(|(n, a)| (n.as_str(), a))
    }

    /// Its plain text for `attrs` (empty without [`RichTextNode::text`]).
    pub fn plain_text(&self, attrs: &NodeAttrs) -> String {
        self.text.as_ref().map(|f| f(attrs)).unwrap_or_default()
    }

    /// Panics when the declaration is invalid.
    fn check(&self) {
        let name = &self.name;
        assert!(
            name.strip_prefix("app.").is_some_and(is_identifier),
            "rich text node `{name}`: names are `app.` then [a-z][a-z0-9_]{{0,31}}"
        );
        assert!(
            self.label.as_deref().is_none_or(|l| !l.trim().is_empty()),
            "rich text node `{name}`: the label is empty"
        );
        assert!(self.attrs.len() <= MAX_NODE_ATTRS, "rich text node `{name}`: more than {MAX_NODE_ATTRS} attributes");
        let mut seen = std::collections::HashSet::new();
        for (attr, decl) in &self.attrs {
            assert!(is_identifier(attr), "rich text node `{name}`: attribute `{attr}` is not [a-z][a-z0-9_]{{0,31}}");
            assert!(seen.insert(attr), "rich text node `{name}`: attribute `{attr}` declared twice");
            // Written as `data-{attr}` beside `data-node`, which names the node.
            assert!(attr != "node", "rich text node `{name}`: the attribute name `node` is reserved");
            match &decl.kind {
                AttrKind::Text { min, max } => assert!(
                    *max >= 1 && *max <= MAX_TEXT_ATTR && min <= max,
                    "rich text node `{name}`: attribute `{attr}`: text bounds must be min ≤ max, 1 ≤ max ≤ {MAX_TEXT_ATTR}"
                ),
                AttrKind::Integer { min, max } => assert!(
                    min <= max && *min >= -MAX_SAFE_INTEGER && *max <= MAX_SAFE_INTEGER,
                    "rich text node `{name}`: attribute `{attr}`: the range must be ordered and within ±(2^53 − 1)"
                ),
                AttrKind::OneOf(options) => {
                    let unique: std::collections::HashSet<_> = options.iter().collect();
                    assert!(
                        !options.is_empty()
                            && options.len() <= MAX_ATTR_OPTIONS
                            && unique.len() == options.len()
                            && options.iter().all(|o| !o.is_empty() && o.chars().count() <= 100),
                        "rich text node `{name}`: attribute `{attr}`: 1 to {MAX_ATTR_OPTIONS} distinct, non-empty options of at most 100 characters"
                    );
                }
                AttrKind::Boolean | AttrKind::Uuid => {}
            }
            if let Some(default) = &decl.default {
                assert!(
                    decl.accept(default).as_ref() == Some(default),
                    "rich text node `{name}`: attribute `{attr}`: the default {default} does not fit its kind"
                );
            }
        }
        // Defaults are stored in every node that leaves them out: kept small.
        let defaults: usize = self
            .attrs
            .iter()
            .filter_map(|(attr, decl)| decl.default.as_ref().map(|d| attr.len() + d.to_string().len()))
            .sum();
        assert!(
            defaults <= MAX_NODE_DEFAULTS,
            "rich text node `{name}`: its defaults take more than {MAX_NODE_DEFAULTS} bytes"
        );
    }

    /// The node's attributes checked and completed (defaults filled in).
    pub(super) fn accept(&self, attrs: &serde_json::Map<String, Value>) -> Result<NodeAttrs, DocumentError> {
        let name = self.name.escape_debug();
        if let Some(extra) = attrs.keys().find(|k| !self.attrs.iter().any(|(n, _)| n == *k)) {
            return Err(err(format!("has an element `{name}` with an invalid attribute `{}`", extra.escape_debug())));
        }
        let mut out = serde_json::Map::new();
        for (attr, decl) in &self.attrs {
            let value = match attrs.get(attr) {
                Some(value) => decl
                    .accept(value)
                    .ok_or_else(|| err(format!("has an element `{name}` with an invalid attribute `{attr}`")))?,
                None => decl
                    .default
                    .clone()
                    .ok_or_else(|| err(format!("has an element `{name}` without its attribute `{attr}`")))?,
            };
            out.insert(attr.clone(), value);
        }
        Ok(NodeAttrs(out))
    }
}

/// `[a-z][a-z0-9_]{0,31}`.
fn is_identifier(name: &str) -> bool {
    let mut bytes = name.bytes();
    bytes.next().is_some_and(|b| b.is_ascii_lowercase())
        && name.len() <= 32
        && bytes.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

/// The application's nodes: what documents may hold beyond the core nodes.
///
/// ```
/// use rustcms_admin::rich_text::{AppNodes, Attr, RichTextNode};
///
/// let nodes = AppNodes::new([RichTextNode::inline_atom("app.kbd").attr("keys", Attr::text())]);
/// let doc = serde_json::json!({"type": "doc", "version": 3, "content": [{"type": "paragraph", "content": [
///     {"type": "app.kbd", "attrs": {"keys": "Ctrl+C"}}
/// ]}]});
/// assert!(nodes.parse(&doc).is_ok());
/// assert!(rustcms_admin::rich_text::parse(&doc).is_err());
/// ```
#[derive(Debug, Clone, Default)]
pub struct AppNodes {
    nodes: Vec<RichTextNode>,
    by_name: HashMap<String, usize>,
}

impl AppNodes {
    /// A table of nodes.
    ///
    /// # Panics
    /// On an invalid declaration (name, attributes, bounds, defaults) or a
    /// name declared twice.
    pub fn new(nodes: impl IntoIterator<Item = RichTextNode>) -> Self {
        let mut table = Self::default();
        for node in nodes {
            table.add(node);
        }
        table
    }

    /// Adds a node (see [`AppNodes::new`]).
    pub(crate) fn add(&mut self, node: RichTextNode) {
        node.check();
        let previous = self.by_name.insert(node.name.clone(), self.nodes.len());
        assert!(previous.is_none(), "rich text node `{}` declared twice", node.name);
        self.nodes.push(node);
    }

    /// The node named `name`.
    pub fn get(&self, name: &str) -> Option<&RichTextNode> {
        self.by_name.get(name).map(|&i| &self.nodes[i])
    }

    /// Every node, in declaration order.
    pub fn iter(&self) -> impl Iterator<Item = &RichTextNode> {
        self.nodes.iter()
    }

    /// Whether there is none.
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Parses a document that may hold any of these nodes (see [`parse`]).
    pub fn parse(&self, value: &Value) -> Result<Document, DocumentError> {
        parse::parse_with(value, Some(Accepts { nodes: self, only: None }))
    }

    /// Parses a document that may hold the nodes named in `only`.
    pub fn parse_for(&self, value: &Value, only: &[&str]) -> Result<Document, DocumentError> {
        let only: Vec<String> = only.iter().map(|s| (*s).to_owned()).collect();
        parse::parse_with(value, Some(Accepts { nodes: self, only: Some(&only) }))
    }

    /// `value` with the defaults of its application nodes filled in, if it
    /// is a document these nodes accept (else `None`).
    pub fn complete(&self, value: &Value) -> Option<Value> {
        // The parser bounds the document as it is once completed.
        let (_, needed) = parse::parse_needed(value, Some(Accepts { nodes: self, only: None })).ok()?;
        let mut out = value.clone();
        self.fill(&mut out);
        lower_version(&mut out, needed);
        Some(out)
    }

    /// Whether `value` holds a node of this table, at any depth.
    pub(crate) fn holds(&self, value: &Value) -> bool {
        let Value::Object(map) = value else { return false };
        map.get("type").and_then(Value::as_str).is_some_and(|name| self.get(name).is_some())
            || map.get("content").and_then(Value::as_array).is_some_and(|c| c.iter().any(|v| self.holds(v)))
    }

    /// Replaces the attributes of every application node of a parsed
    /// document by their accepted form (defaults filled in, UUIDs in lower
    /// case); the rest is left as it is.
    pub(crate) fn fill(&self, value: &mut Value) {
        let Value::Object(map) = value else { return };
        if let Some(node) = map.get("type").and_then(Value::as_str).and_then(|name| self.get(name)) {
            let attrs = map.get("attrs").and_then(Value::as_object).cloned().unwrap_or_default();
            if let Ok(accepted) = node.accept(&attrs) {
                map.insert("attrs".into(), Value::Object(accepted.0));
            }
            return;
        }
        if let Some(Value::Array(content)) = map.get_mut("content") {
            for child in content {
                self.fill(child);
            }
        }
    }
}

/// What a document may hold beyond the core nodes: some of a table's nodes.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Accepts<'a> {
    pub nodes: &'a AppNodes,
    /// Names accepted (`None`: every node of the table).
    pub only: Option<&'a [String]>,
}

impl<'a> Accepts<'a> {
    /// The declaration of `name`, if accepted.
    pub(super) fn get(&self, name: &str) -> Option<&'a RichTextNode> {
        self.nodes.get(name).filter(|_| self.only.is_none_or(|only| only.iter().any(|n| n == name)))
    }
}

/// Writes `value` (a parsed document) in the lowest version its parts need.
pub(crate) fn lower_version(value: &mut Value, needed: u64) {
    if let Some(version) = value.get_mut("version")
        && version.as_u64().is_some_and(|v| v > needed)
    {
        *version = Value::from(needed);
    }
}
