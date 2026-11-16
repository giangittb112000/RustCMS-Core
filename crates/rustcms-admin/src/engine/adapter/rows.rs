//! Relationship repeaters, the child's side (ADR 0040): the rows a parent's
//! write changes, written in the parent's transaction with the child's own
//! checks (scope, view policy under the lock, field rules, references,
//! policy per record, hooks, history).

use super::*;

/// What a relationship repeater changes, its shape already checked.
#[derive(Debug, Clone, Default)]
pub(crate) struct RowChange {
    /// New rows: `(ref, data)`.
    pub(crate) create: Vec<(String, Data)>,
    /// Changed rows: `(key, data)`.
    pub(crate) update: Vec<(String, Data)>,
    /// Removed rows (to the trash with soft deletes).
    pub(crate) delete: Vec<String>,
    /// Every row the user sees after the change, keys and refs, in order.
    pub(crate) order: Option<Vec<String>>,
}

/// Where the rows belong.
pub(crate) struct RowTarget<'a> {
    /// The parent resource (its slug) and the relation's name (error paths).
    pub(crate) parent_slug: &'a str,
    pub(crate) relation: &'a str,
    /// The child's field holding the parent's key, and that key.
    pub(crate) field: &'a str,
    pub(crate) parent_key: &'a str,
    /// The child's integer field holding each row's position.
    pub(crate) order_column: Option<&'a str>,
    /// The repeater's bounds on the parent's rows (checked when rows are added or removed).
    pub(crate) min_items: Option<u32>,
    pub(crate) max_items: Option<u32>,
}

/// What changed, for the parent's history.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct RowsOutcome {
    pub(crate) created: Vec<String>,
    pub(crate) updated: Vec<String>,
    pub(crate) deleted: Vec<String>,
    pub(crate) reordered: bool,
}

impl RowsOutcome {
    pub(crate) fn is_empty(&self) -> bool {
        self.created.is_empty() && self.updated.is_empty() && self.deleted.is_empty() && !self.reordered
    }
}

/// `errors` of row `id` moved below `relation.id.` (`quantity` → `lines.8.quantity`).
fn below(relation: &str, id: &str, err: AppError) -> AppError {
    match err {
        AppError::Validation(errors) => {
            let mut out = ValidationErrors::new();
            for (field, messages) in errors.errors {
                for message in messages {
                    out.add(format!("{relation}.{id}.{field}"), message);
                }
            }
            for message in errors.form_errors {
                out.add(format!("{relation}.{id}"), message);
            }
            AppError::Validation(out)
        }
        other => other,
    }
}

impl<R: Resource> ResourceAdapter<R> {
    /// Writes `change` to the children of `target`, in `txn` (the parent's
    /// write, `ctx` its context). Rows named by key are locked in key order
    /// through the child's scope and view policy (re-checked under the lock);
    /// one not visible or not a child of this parent is a 422 at
    /// `relation.key`. Then deletes, updates, creates, positions.
    pub(super) async fn write_rows_in(
        &self,
        ctx: &Ctx,
        txn: &Arc<sea_orm::DatabaseTransaction>,
        target: RowTarget<'_>,
        change: RowChange,
    ) -> AppResult<RowsOutcome> {
        let relation = target.relation;
        let parent_value = self.fk_value(target.field, target.parent_key);
        let mut named: Vec<String> =
            change.update.iter().map(|(k, _)| k.clone()).chain(change.delete.clone()).collect();
        named.sort();
        named.dedup();
        let visible = self.locked_visible(ctx, txn, &named, &[]).await?;
        let mut errors = ValidationErrors::new();
        let mut models = HashMap::new();
        for key in &named {
            let found = if visible.contains(key) {
                self.find_model_in(ctx, txn.as_ref(), key, Trashed::Without).await.ok()
            } else {
                None
            };
            match found {
                Some(model) if self.record(&model).get(target.field) == Some(&parent_value) => {
                    models.insert(key.clone(), model);
                }
                _ => errors.add(format!("{relation}.{key}"), "is not one of the rows"),
            }
        }
        errors.into_result()?;

        let mut outcome = RowsOutcome::default();
        // Deletes: the child's policy on each locked row.
        for key in &change.delete {
            let model = models.remove(key).expect("locked above");
            if !self.policy.allows(ctx, ability::DELETE, Some(&self.record(&model))).await {
                return Err(AppError::Forbidden);
            }
            self.delete_locked(ctx, txn, model).await?;
            outcome.deleted.push(key.clone());
        }
        // Updates: the child's field rules, conditions, hooks, references, history.
        for (key, data) in change.update {
            let model = models.remove(&key).expect("locked above");
            if !self.policy.allows(ctx, ability::UPDATE, Some(&self.record(&model))).await {
                return Err(AppError::Forbidden);
            }
            if data.contains_key(target.field) || target.order_column.is_some_and(|c| data.contains_key(c)) {
                return Err(ValidationErrors::field(format!("{relation}.{key}"), "is not allowed").into());
            }
            let changed =
                self.update_row(ctx, txn, model, data, target.field).await.map_err(|e| below(relation, &key, e))?;
            if changed {
                outcome.updated.push(key);
            }
        }
        // Creates, with the parent in context; refs get their keys.
        let mut refs = HashMap::new();
        let mut bound = ctx.clone();
        bound.bound = Some((target.parent_slug.to_owned(), target.field.to_owned(), target.parent_key.to_owned()));
        if !change.create.is_empty() && !self.policy.allows(&bound, ability::CREATE, None).await {
            return Err(AppError::Forbidden);
        }
        // New rows go after every row of the parent (an order may move them next).
        let mut next = match target.order_column {
            Some(column) => self.largest_position(txn, &target, column, &parent_value).await? + 1,
            None => 0,
        };
        for (reference, mut data) in change.create {
            if data.contains_key(target.field) || target.order_column.is_some_and(|c| data.contains_key(c)) {
                return Err(ValidationErrors::field(format!("{relation}.{reference}"), "is not allowed").into());
            }
            data.insert(target.field.to_owned(), parent_value.clone());
            let position = target.order_column.map(|column| {
                next += 1;
                (column, next - 1)
            });
            let key = self.create_row(&bound, txn, data, position).await.map_err(|e| below(relation, &reference, e))?;
            refs.insert(reference, key.clone());
            outcome.created.push(key);
        }
        // The repeater's bounds hold on the parent's rows, whatever the client checked.
        let min = target.min_items.filter(|_| !outcome.deleted.is_empty());
        let max = target.max_items.filter(|_| !outcome.created.is_empty());
        if min.is_some() || max.is_some() {
            let count = self.live_rows(&target, &parent_value)?.count(txn.as_ref()).await?;
            if let Some(min) = min.filter(|m| count < u64::from(*m)) {
                return Err(ValidationErrors::field(relation, format!("must have at least {min} items")).into());
            }
            if let Some(max) = max.filter(|m| count > u64::from(*m)) {
                return Err(ValidationErrors::field(relation, format!("may not have more than {max} items")).into());
            }
        }
        if let (Some(column), Some(order)) = (target.order_column, change.order) {
            outcome.reordered = self.reorder_rows(ctx, txn, &target, column, &parent_value, &order, &refs).await?;
        }
        Ok(outcome)
    }

    /// Deletes a locked record (the trash with soft deletes), with its history and hooks.
    pub(super) async fn delete_locked(
        &self,
        ctx: &Ctx,
        txn: &Arc<sea_orm::DatabaseTransaction>,
        model: Model<R>,
    ) -> AppResult<()> {
        let event = if self.soft.is_some() { "trashed" } else { "deleted" };
        self.audit_write(ctx, txn, event, &self.key_of(&model), None, None).await?;
        if self.soft.is_some() {
            // To the trash: related records are kept (no database cascade runs).
            let mut active = model.into_active_model();
            self.set_marker(&mut active, true)?;
            let model = active.update(txn.as_ref()).await.map_err(|e| self.map_db_err(ctx, e))?;
            R::after_trash(ctx, txn, &model).await
        } else {
            Entity::<R>::delete_many()
                .filter(self.pk.eq(model.get(self.pk)))
                .exec(txn.as_ref())
                .await
                .map_err(|e| self.map_db_err(ctx, e))?;
            R::after_delete(ctx, txn, &model).await
        }
    }

    /// Updates a locked row with client `data`; whether anything was written.
    async fn update_row(
        &self,
        ctx: &Ctx,
        txn: &Arc<sea_orm::DatabaseTransaction>,
        model: Model<R>,
        data: Data,
        parent_field: &str,
    ) -> AppResult<bool> {
        let mut input = self.accept_input(ctx, Operation::Update, data)?;
        self.apply_conditions(ctx, Operation::Update, &mut input, Some(&self.record(&model)))?;
        let data = R::mutate_before_update(ctx, txn, &model, input).await?;
        // The row stays the child of the parent that was locked and checked.
        if data.get(parent_field).is_some_and(|v| self.record(&model).get(parent_field) != Some(v)) {
            return Err(AppError::internal(format!("`{}`: a hook changed the relation key `{parent_field}`", R::SLUG)));
        }
        self.check_written(ctx, txn, &data).await?;
        if data.is_empty() {
            return Ok(false);
        }
        let before = self.audit_snapshot(&model);
        let stored = model.clone();
        let mut active = model.into_active_model();
        self.apply(ctx, &mut active, &data, Operation::Update, Some(&stored))?;
        let model = active.update(txn.as_ref()).await.map_err(|e| self.map_db_err(ctx, e))?;
        R::after_save(ctx, txn, &model).await?;
        self.audit_outcome(ctx, txn, "updated", Some(&before), &model, false, false).await?;
        Ok(true)
    }

    /// Inserts a row (`data` holds the parent key); its key.
    async fn create_row(
        &self,
        ctx: &Ctx,
        txn: &Arc<sea_orm::DatabaseTransaction>,
        data: Data,
        position: Option<(&str, i64)>,
    ) -> AppResult<String> {
        let mut input = self.accept_input(ctx, Operation::Create, data)?;
        // The position is the server's (never client input).
        if let Some((column, value)) = position {
            input.insert(column.to_owned(), serde_json::Value::from(value));
        }
        self.apply_conditions(ctx, Operation::Create, &mut input, None)?;
        let parents = self.lock_parents(ctx, txn, self.parent_of_data(&input).flatten()).await?;
        let (_, field, _) = ctx.bound.clone().expect("rows are created with their parent");
        let parent_value = input.get(&field).cloned();
        let data = R::mutate_before_create(ctx, txn, input).await?;
        self.check_parent_written(ctx, &data, &parents)?;
        if data.get(&field) != parent_value.as_ref() {
            return Err(AppError::internal(format!("`{}`: a hook changed the relation key `{field}`", R::SLUG)));
        }
        self.check_written(ctx, txn, &data).await?;
        let mut active = <<Entity<R> as EntityTrait>::ActiveModel as ActiveModelBehavior>::new();
        self.apply(ctx, &mut active, &data, Operation::Create, None)?;
        self.set_marker(&mut active, false)?;
        match self.meta.key {
            KeyStrategy::DbGenerated => active.not_set(self.pk),
            KeyStrategy::AppGenerated(generate) => active
                .try_set(self.pk, generate())
                .map_err(|e| AppError::internal(format!("`{}` key: {e}", R::SLUG)))?,
            KeyStrategy::Natural if active.is_not_set(self.pk) => {
                return Err(ValidationErrors::field(self.pk_name.as_str(), "is required").into());
            }
            KeyStrategy::Natural => {}
        }
        let model = active.insert(txn.as_ref()).await.map_err(|e| self.map_db_err(ctx, e))?;
        self.check_parent_stored(ctx, &model, &parents)?;
        R::after_save(ctx, txn, &model).await?;
        self.audit_outcome(ctx, txn, "created", None, &model, false, true).await?;
        Ok(self.key_of(&model))
    }

    /// The parent's rows not in the trash.
    fn live_rows(
        &self,
        target: &RowTarget<'_>,
        parent_value: &serde_json::Value,
    ) -> AppResult<sea_orm::Select<Entity<R>>> {
        let mut query = Entity::<R>::find().filter(self.parent_filter(target, parent_value)?);
        if let Some((marker, _, _)) = &self.soft {
            query = query.filter(marker.is_null());
        }
        Ok(query)
    }

    /// `target.field` equals the parent's key.
    fn parent_filter(&self, target: &RowTarget<'_>, parent_value: &serde_json::Value) -> AppResult<sea_orm::Condition> {
        let fk = self.column(target.field);
        let parent = fk
            .kind
            .decode(parent_value)
            .map_err(|e| AppError::internal(e.0))?
            .into_iter()
            .next()
            .ok_or_else(|| AppError::internal("a parent key without a value"))?;
        Ok(sea_orm::Condition::all().add(fk.column.eq(parent)))
    }

    /// The largest position among the parent's rows (trashed ones included:
    /// their places stay taken), 0 for none.
    async fn largest_position(
        &self,
        txn: &Arc<sea_orm::DatabaseTransaction>,
        target: &RowTarget<'_>,
        column: &str,
        parent_value: &serde_json::Value,
    ) -> AppResult<i64> {
        let position = self.column(column).column;
        let rows = Entity::<R>::find().filter(self.parent_filter(target, parent_value)?).all(txn.as_ref()).await?;
        Ok(rows
            .iter()
            .filter_map(|m| encode_column(position.def().get_column_type(), &m.get(position)).as_i64())
            .max()
            .unwrap_or(0))
    }

    /// Writes the positions `order` sets (ADR 0040): the rows the user sees
    /// take, in that order, the positions those rows held (new rows the ones
    /// after the largest); rows the user cannot see keep theirs. Two passes,
    /// so a unique `(parent, position)` holds throughout. Whether a position
    /// changed.
    #[allow(clippy::too_many_arguments)]
    async fn reorder_rows(
        &self,
        ctx: &Ctx,
        txn: &Arc<sea_orm::DatabaseTransaction>,
        target: &RowTarget<'_>,
        column: &str,
        parent_value: &serde_json::Value,
        order: &[String],
        refs: &HashMap<String, String>,
    ) -> AppResult<bool> {
        let relation = target.relation;
        let position = self.column(column).column;
        let decode = |kind: FieldKind, value: &serde_json::Value| -> AppResult<Vec<Value>> {
            kind.decode(value).map_err(|e| AppError::internal(e.0))
        };
        // Every row of the parent (locked), and those the user sees now.
        let query = self.live_rows(target, parent_value)?;
        let all: Vec<Model<R>> = query.order_by_asc(self.pk).lock_exclusive().all(txn.as_ref()).await?;
        let keys: Vec<String> = all.iter().map(|m| self.key_of(m)).collect();
        let visible = self.locked_visible(ctx, txn, &keys, &[]).await?;
        let mut wanted: Vec<String> = Vec::with_capacity(order.len());
        let mut errors = ValidationErrors::new();
        for id in order {
            let key = refs.get(id).cloned().unwrap_or_else(|| id.clone());
            if !visible.contains(&key) || wanted.contains(&key) {
                errors.add(relation, "must list every row once");
                break;
            }
            wanted.push(key);
        }
        if wanted.len() != visible.len() {
            errors.add(relation, "must list every row once");
        }
        errors.into_result()?;
        let held = |m: &Model<R>| encode_column(position.def().get_column_type(), &m.get(position)).as_i64();
        // The places the visible rows hold (new rows were given the ones after the largest).
        let mut slots: Vec<i64> = all.iter().filter(|m| visible.contains(&self.key_of(m))).filter_map(held).collect();
        slots.sort_unstable();
        // Trashed rows keep their places: the moves go above them too.
        let mut largest = self.largest_position(txn, target, column, parent_value).await?;
        while slots.len() < wanted.len() {
            largest += 1;
            slots.push(largest);
        }
        let by_key: HashMap<String, &Model<R>> = all.iter().map(|m| (self.key_of(m), m)).collect();
        let moves: Vec<(&Model<R>, i64)> = wanted
            .iter()
            .zip(slots)
            .filter_map(|(key, slot)| by_key.get(key).map(|m| (*m, slot)))
            .filter(|(m, slot)| held(m) != Some(*slot))
            .collect();
        if moves.is_empty() {
            return Ok(false);
        }
        // Moving a stored row writes it: `update` on the child and on each row (new rows are the user's).
        let created: HashSet<&String> = refs.values().collect();
        let stored: Vec<&Model<R>> =
            moves.iter().map(|(m, _)| *m).filter(|m| !created.contains(&self.key_of(m))).collect();
        if !stored.is_empty() && !self.policy.admits(ctx, ability::UPDATE).await {
            return Err(AppError::Forbidden);
        }
        for model in stored {
            if !self.policy.allows(ctx, ability::UPDATE, Some(&self.record(model))).await {
                return Err(AppError::Forbidden);
            }
        }
        // Out of the way first (above every position in use), then in place.
        let offset = largest + 1;
        let kind = self.column(column).kind;
        for pass in [true, false] {
            for (index, (model, slot)) in moves.iter().enumerate() {
                let value = if pass { offset + index as i64 } else { *slot };
                let mut active = (*model).clone().into_active_model();
                let candidates = decode(kind, &serde_json::Value::from(value))?;
                if !candidates.into_iter().any(|v| active.try_set(position, v).is_ok()) {
                    return Err(AppError::internal(format!("`{}`: cannot write the order column `{column}`", R::SLUG)));
                }
                active.update(txn.as_ref()).await.map_err(|e| self.map_db_err(ctx, e))?;
            }
        }
        Ok(true)
    }
}

/// Most entries of one part of a relationship repeater's change.
const MAX_ROW_ITEMS: usize = 100;

/// A relationship repeater of a resource's form, checked at boot.
#[derive(Debug, Clone)]
pub(crate) struct RowRepeater {
    /// The has-many relation (and the input's name).
    pub(crate) name: &'static str,
    pub(crate) target: &'static str,
    /// The child's field holding the parent key.
    pub(crate) field: &'static str,
    pub(crate) order_column: Option<String>,
    pub(crate) spec: crate::ui::RepeaterSpec,
    /// The child fields the rows edit.
    pub(crate) inputs: Vec<String>,
}

/// The relationship repeaters of `form`; panics on what cannot hold here
/// (the child's fields are checked by `Registry::finish`).
pub(super) fn row_repeaters(slug: &str, form: &Form, relations: &[Relation]) -> Vec<RowRepeater> {
    let mut out = Vec::new();
    for input in form.inputs() {
        let Some(spec) = input.repeater.as_deref().filter(|s| s.relationship) else { continue };
        let name = input.name.as_str();
        let here = format!("resource `{slug}`: repeater `{name}`");
        let relation = relations
            .iter()
            .find(|r| r.name == name && r.pivot.is_none())
            .unwrap_or_else(|| panic!("{here}: `relationship()` needs a has-many relation named `{name}`"));
        crate::engine::repeater::check_spec(&here, spec);
        assert!(
            input.shown_when.is_none() && !input.clear_when_hidden,
            "{here}: conditions on relationship repeaters are not supported yet"
        );
        let mut inputs = Vec::new();
        fn conditions(components: &[Component]) -> bool {
            components.iter().any(|c| match c {
                Component::Input(i) => i.shown_when.is_some(),
                Component::Layout(l) => l.shown_when.is_some() || conditions(&l.children),
                Component::Entry(_) => false,
            })
        }
        assert!(!conditions(&spec.components), "{here}: conditions in relationship rows are not supported yet");
        for row in crate::engine::repeater::row_inputs(&spec.components) {
            let row_name = row.name.as_str();
            assert!(
                !matches!(
                    row.kind,
                    InputType::Repeater | InputType::BelongsToMany | InputType::Secret | InputType::SecretMap
                ),
                "{here}: `{row_name}`: repeaters, belongs-to-many and secret inputs cannot be in relationship rows"
            );
            assert!(
                !row.required && row.max_length.is_none() && !row.integer,
                "{here}: `{row_name}`: in relationship rows the child's field decides `required`, `max_length` and \
                 integers"
            );
            assert!(!inputs.iter().any(|n| n == row_name), "{here}: input `{row_name}` appears twice in a row");
            inputs.push(row_name.to_owned());
        }
        if let Some(column) = &spec.order_column {
            assert!(!inputs.contains(column), "{here}: the order column `{column}` cannot be an input");
        }
        assert!(
            !inputs.iter().any(|n| n == relation.field),
            "{here}: the parent key `{}` cannot be an input",
            relation.field
        );
        out.push(RowRepeater {
            name: relation.name,
            target: relation.target,
            field: relation.field,
            order_column: spec.order_column.clone(),
            spec: spec.clone(),
            inputs,
        });
    }
    out
}

impl<R: Resource> ResourceAdapter<R> {
    /// The relationship repeaters' changes in `input` (removed from it):
    /// `create`, `update` and `delete` on the child admitted for the parts sent
    /// (403) before any key is read, then the shape (422 on the repeater).
    pub(super) async fn take_rows(
        &self,
        ctx: &Ctx,
        op: Operation,
        input: &mut Data,
    ) -> AppResult<Vec<(usize, RowChange)>> {
        let mut sent = Vec::new();
        for (index, repeater) in self.row_repeaters.iter().enumerate() {
            if let Some(value) = input.remove(repeater.name) {
                sent.push((index, value));
            }
        }
        let filled = |value: &serde_json::Value, part: &str| {
            value.get(part).and_then(|v| v.as_array()).is_some_and(|a| !a.is_empty())
        };
        for (index, value) in &sent {
            let repeater = &self.row_repeaters[*index];
            let child = ctx.registry.get(repeater.target).ok_or(AppError::NotFound)?;
            for (part, needed) in
                [("create", ability::CREATE), ("update", ability::UPDATE), ("delete", ability::DELETE)]
            {
                if filled(value, part) && !child.admits(ctx, needed).await {
                    return Err(AppError::Forbidden);
                }
            }
        }
        let mut errors = ValidationErrors::new();
        let mut out = Vec::new();
        for (index, value) in sent {
            let repeater = &self.row_repeaters[index];
            let name = repeater.name;
            let child = ctx.registry.get(repeater.target).ok_or(AppError::NotFound)?;
            match parse_change(value, op, child.as_ref()) {
                Some(change) => out.push((index, change)),
                None => errors.add(name, "is invalid"),
            }
        }
        errors.into_result()?;
        Ok(out)
    }

    /// Writes the rows' changes after the parent's row (`model`) is written.
    pub(super) async fn write_row_changes(
        &self,
        ctx: &Ctx,
        txn: &Arc<sea_orm::DatabaseTransaction>,
        model: &Model<R>,
        changes: Vec<(usize, RowChange)>,
    ) -> AppResult<Vec<(&'static str, RowsOutcome)>> {
        let key = self.key_of(model);
        let mut out = Vec::new();
        for (index, change) in changes {
            let repeater = &self.row_repeaters[index];
            let child = ctx.registry.get(repeater.target).ok_or(AppError::NotFound)?;
            let target = RowTarget {
                parent_slug: R::SLUG,
                relation: repeater.name,
                field: repeater.field,
                parent_key: &key,
                order_column: repeater.order_column.as_deref(),
                min_items: repeater.spec.min_items,
                max_items: repeater.spec.max_items,
            };
            out.push((repeater.name, child.write_rows(ctx, txn, target, change).await?));
        }
        Ok(out)
    }

    /// The parent's history entries of its rows' changes, one per relation
    /// (each child has its own entries).
    pub(super) async fn audit_rows(
        &self,
        ctx: &Ctx,
        txn: &sea_orm::DatabaseTransaction,
        model: &Model<R>,
        outcomes: Vec<(&'static str, RowsOutcome)>,
    ) -> AppResult<()> {
        if !self.meta.audit {
            return Ok(());
        }
        let key = self.key_of(model);
        let entries: Vec<_> = outcomes
            .into_iter()
            .filter(|(_, o)| !o.is_empty())
            .map(|(relation, o)| {
                rustcms_core::audit::AuditEntry::new("rows_changed", R::SLUG).key(&key).meta(serde_json::json!({
                    "relation": relation,
                    "created": o.created,
                    "updated": o.updated,
                    "deleted": o.deleted,
                    "reordered": o.reordered,
                }))
            })
            .collect();
        rustcms_core::audit::record_all(txn, &ctx.audit_context, entries).await
    }
}

/// A relationship repeater's change, its shape checked (`None`: invalid).
fn parse_change(value: serde_json::Value, op: Operation, child: &dyn DynResource) -> Option<RowChange> {
    let object = value.as_object()?;
    if !object.keys().all(|k| matches!(k.as_str(), "create" | "update" | "delete" | "order")) {
        return None;
    }
    let list = |part: &str| -> Option<Vec<serde_json::Value>> {
        match object.get(part) {
            None => Some(Vec::new()),
            Some(serde_json::Value::Array(items)) if items.len() <= MAX_ROW_ITEMS => Some(items.clone()),
            Some(_) => None,
        }
    };
    let (create, update, delete) = (list("create")?, list("update")?, list("delete")?);
    if op == Operation::Create && (!update.is_empty() || !delete.is_empty()) {
        return None;
    }
    let valid_ref = |r: &str| {
        (1..=16).contains(&r.len()) && r.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    };
    let mut change = RowChange::default();
    let mut refs = HashSet::new();
    for item in create {
        let reference = item.get("ref")?.as_str().filter(|r| valid_ref(r))?.to_owned();
        let data = item.get("data")?.as_object()?.clone();
        if !refs.insert(reference.clone()) {
            return None;
        }
        change.create.push((reference, data));
    }
    let mut keys = HashSet::new();
    for item in update {
        let key = child.canonical_key(item.get("key")?.as_str()?)?;
        let data = item.get("data")?.as_object()?.clone();
        if !keys.insert(key.clone()) {
            return None;
        }
        change.update.push((key, data));
    }
    for item in delete {
        let key = child.canonical_key(item.as_str()?)?;
        if !keys.insert(key.clone()) {
            return None;
        }
        change.delete.push(key);
    }
    if let Some(order) = object.get("order") {
        let items = order.as_array().filter(|a| a.len() <= 10 * MAX_ROW_ITEMS)?;
        let mut ids = Vec::with_capacity(items.len());
        for item in items {
            let id = item.as_str()?;
            ids.push(if refs.contains(id) { id.to_owned() } else { child.canonical_key(id)? });
        }
        change.order = Some(ids);
    }
    Some(change)
}
