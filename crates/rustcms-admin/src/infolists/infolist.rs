//! The infolist of a record's view page, and computed values.

use std::sync::Arc;
use std::time::Duration;

use rustcms_core::AppResult;
use sea_orm::EntityTrait;
use serde_json::Value;

use crate::action::BoxFuture;
use crate::context::Ctx;
use crate::ui::{Component, FormWidth};

/// What a record's view page shows (ADR 0039): entries in the form's layouts.
/// Empty (the default): entries made from the form, read-only. Declared: it
/// replaces the form on the view page.
#[derive(Debug, Clone)]
pub struct Infolist {
    pub(crate) columns: u8,
    pub(crate) components: Vec<Component>,
    pub(crate) width: Option<FormWidth>,
}

impl Default for Infolist {
    fn default() -> Self {
        Self::new()
    }
}

impl Infolist {
    /// Empty one-column infolist.
    pub fn new() -> Self {
        Self { columns: 1, components: Vec::new(), width: None }
    }

    /// Number of columns.
    pub fn columns(mut self, columns: u8) -> Self {
        self.columns = columns.max(1);
        self
    }

    /// Entries and layouts (`components![…]`).
    pub fn schema(mut self, components: Vec<Component>) -> Self {
        self.components = components;
        self
    }

    /// How wide the view page is (default: the form's width).
    pub fn width(mut self, width: FormWidth) -> Self {
        self.width = Some(width);
        self
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.components.is_empty()
    }

    /// Every entry, depth first.
    pub(crate) fn entries(&self) -> Vec<&super::Entry> {
        fn walk<'a>(components: &'a [Component], out: &mut Vec<&'a super::Entry>) {
            for component in components {
                match component {
                    Component::Entry(entry) => out.push(entry),
                    Component::Layout(layout) => walk(&layout.children, out),
                    Component::Input(_) => {}
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.components, &mut out);
        out
    }
}

type Compute<E> =
    Arc<dyn for<'a> Fn(&'a Ctx, &'a <E as EntityTrait>::Model) -> BoxFuture<'a, AppResult<Value>> + Send + Sync>;

/// Time a computed value may take by default.
pub const COMPUTED_TIMEOUT: Duration = Duration::from_secs(5);
/// The longest [`Computed::timeout`].
pub const COMPUTED_MAX_TIMEOUT: Duration = Duration::from_secs(30);

/// A value computed on the server when a record's view page opens, shown by a
/// [`ComputedEntry`](super::ComputedEntry) of the same name (ADR 0039). Never
/// computed for lists, exports or writes; never stored.
pub struct Computed<E: EntityTrait> {
    pub(crate) name: String,
    pub(crate) compute: Compute<E>,
    pub(crate) readable: Option<crate::field::Rule>,
    pub(crate) timeout: Duration,
}

impl<E: EntityTrait> Clone for Computed<E> {
    fn clone(&self) -> Self {
        Self {
            name: self.name.clone(),
            compute: self.compute.clone(),
            readable: self.readable.clone(),
            timeout: self.timeout,
        }
    }
}

impl<E: EntityTrait> std::fmt::Debug for Computed<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Computed").field("name", &self.name).field("timeout", &self.timeout).finish_non_exhaustive()
    }
}

impl<E: EntityTrait> Computed<E> {
    /// The value `name`, computed from the record by `compute` (keep it light:
    /// it runs each time the page opens). It fails alone: an error, a timeout
    /// or a value over 64 KiB shows "not available" in its entry.
    pub fn make<F>(name: impl Into<String>, compute: F) -> Self
    where
        F: for<'a> Fn(&'a Ctx, &'a E::Model) -> BoxFuture<'a, AppResult<Value>> + Send + Sync + 'static,
    {
        Self { name: name.into(), compute: Arc::new(compute), readable: None, timeout: COMPUTED_TIMEOUT }
    }

    /// Who may see the value (default: whoever may view the record). An
    /// aggregate can disclose more than the fields it reads: restrict it as
    /// you would a field.
    pub fn readable(mut self, rule: impl Fn(&Ctx) -> bool + Send + Sync + 'static) -> Self {
        self.readable = Some(Arc::new(rule));
        self
    }

    /// How long the value may take (default 5 seconds, at most 30).
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub(crate) fn can_read(&self, ctx: &Ctx) -> bool {
        self.readable.as_ref().is_none_or(|rule| rule(ctx))
    }
}
