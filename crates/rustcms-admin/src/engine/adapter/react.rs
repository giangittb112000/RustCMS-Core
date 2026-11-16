//! Form reactions (ADR 0041): boot checks, the `form/react` route and strict
//! reactions on save.

use std::time::Duration;

use super::*;
use crate::forms::{ReactionFn, is_empty};
use crate::schema::{FormReactRequest, FormReactResponse};
use crate::ui::{FormPatch, FormState, REACTION_MAX_TIMEOUT};

/// Reactions running at once, across users (saves are not bounded here).
static REACTING: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(8);
/// Longest wait for a turn before answering 429 (the user is typing).
const REACTING_WAIT: Duration = Duration::from_secs(5);

/// The reaction of an input, checked at boot.
pub(super) struct InputReaction {
    pub(super) name: String,
    run: ReactionFn,
    strict: bool,
    timeout: Duration,
}

/// The reactions of `form`, in form order; panics on what cannot hold (ADR
/// 0041, boot checks). Rows refuse reactions themselves (`check_spec`).
pub(super) fn reactions(slug: &str, form: &Form, fields: &[Field]) -> Vec<InputReaction> {
    let mut out = Vec::new();
    for input in form.inputs() {
        let Some(reaction) = input.reaction.as_deref() else { continue };
        let name = input.name.as_str();
        let here = format!("resource `{slug}`: input `{name}`");
        let run = reaction
            .run
            .clone()
            .unwrap_or_else(|| panic!("{here}: `.strict()` and `.reaction_timeout()` need `.reacts()`"));
        assert!(
            !reaction.timeout.is_zero() && reaction.timeout <= REACTION_MAX_TIMEOUT,
            "{here}: the reaction timeout must be above zero and at most {REACTION_MAX_TIMEOUT:?}"
        );
        let relationship = input.repeater.as_deref().is_some_and(|s| s.relationship);
        assert!(
            fields.iter().any(|f| f.name == name)
                && !relationship
                && !matches!(
                    input.kind,
                    InputType::Hidden | InputType::Secret | InputType::SecretMap | InputType::BelongsToMany
                ),
            "{here}: only inputs of fields react (not hidden, encrypted, link or relationship repeater inputs)"
        );
        out.push(InputReaction { name: name.to_owned(), run, strict: reaction.strict, timeout: reaction.timeout });
    }
    out
}

impl<R: Resource> ResourceAdapter<R> {
    /// `POST form/react`: the changes the reaction of `request.changed` asks
    /// for, kept when the user could make them. Authorized as the form's
    /// write: `create` without a record, `edit` on a record in scope.
    pub(super) async fn react(&self, ctx: &Ctx, request: FormReactRequest) -> AppResult<FormReactResponse> {
        let (op, stored) = match (request.operation, request.key.as_deref()) {
            (SchemaOperation::Create, None) => {
                if !self.policy.allows(ctx, ability::CREATE, None).await {
                    return Err(AppError::Forbidden);
                }
                (Operation::Create, None)
            }
            (SchemaOperation::Edit, Some(key)) => {
                self.admit(ctx, ability::UPDATE).await?;
                let model = self.find_model(ctx, &ctx.db, key).await?;
                let record = self.record(&model);
                if !self.policy.allows(ctx, ability::UPDATE, Some(&record)).await {
                    return Err(AppError::Forbidden);
                }
                (Operation::Update, Some(record))
            }
            _ => return Err(ValidationErrors::field("operation", "is invalid").into()),
        };
        let reaction = self
            .reactions
            .iter()
            .find(|r| r.name == request.changed && self.writable(ctx, op, &r.name))
            .ok_or_else(|| AppError::from(ValidationErrors::field("changed", "is invalid")))?;
        let empty = |failed| FormReactResponse { seq: request.seq, patches: Vec::new(), failed };
        // What the form holds: sent values the user may read that decode (the
        // form may be half typed), over the stored record or the defaults.
        let mut resulting = stored.unwrap_or_else(|| self.defaults());
        for (name, value) in request.state {
            if self.state_field(ctx, &name) && (value.is_null() || self.column(&name).kind.decode(&value).is_ok()) {
                resulting.insert(name, value);
            }
        }
        let hidden = self.conditions.hidden(&resulting);
        if hidden.contains(&reaction.name) {
            return Ok(empty(false));
        }
        // A bounded wait for a turn, after authorization and before running
        // anything (the route is also limited per user, see `http::react`).
        let _turn = tokio::time::timeout(REACTING_WAIT, REACTING.acquire())
            .await
            .map_err(|_| AppError::TooManyRequests { retry_after: 5 })?
            .map_err(AppError::internal)?;
        let state = FormState {
            operation: op,
            key: request.key.clone(),
            changed: reaction.name.clone(),
            values: self.readable_state(ctx, &resulting),
        };
        match self.run_reaction(ctx, reaction, &state).await {
            Ok(patches) => Ok(FormReactResponse {
                seq: request.seq,
                patches: self.keep_patches(ctx, op, &state, &hidden, patches),
                failed: false,
            }),
            Err(failure) => {
                tracing::warn!(resource = R::SLUG, input = reaction.name, failure, "form reaction failed");
                Ok(empty(true))
            }
        }
    }

    /// Strict reactions (ADR 0041) on the values a write results in (`input`
    /// over the locked record, or over the columns' defaults on create):
    /// "is not up to date" while one still has changes the user could make,
    /// "could not be checked" when one fails. Runs after the conditions,
    /// before the hooks, on every write path.
    pub(super) async fn check_strict(
        &self,
        ctx: &Ctx,
        op: Operation,
        key: Option<&str>,
        input: &Data,
        stored: Option<&Data>,
    ) -> AppResult<()> {
        if !self.reactions.iter().any(|r| r.strict) {
            return Ok(());
        }
        let mut resulting = stored.cloned().unwrap_or_else(|| self.defaults());
        resulting.extend(input.iter().map(|(k, v)| (k.clone(), v.clone())));
        let hidden = self.conditions.hidden(&resulting);
        let values = self.readable_state(ctx, &resulting);
        let mut errors = ValidationErrors::new();
        for reaction in self.reactions.iter().filter(|r| r.strict && !hidden.contains(&r.name)) {
            let state = FormState {
                operation: op,
                key: key.map(str::to_owned),
                changed: reaction.name.clone(),
                values: values.clone(),
            };
            match self.run_reaction(ctx, reaction, &state).await {
                Ok(patches) => {
                    let kept = self.keep_patches(ctx, op, &state, &hidden, patches);
                    if kept.iter().any(|p| !matches!(p, FormPatch::Notice { .. })) {
                        errors.add(reaction.name.as_str(), "is not up to date");
                    }
                }
                Err(failure) => {
                    tracing::warn!(resource = R::SLUG, input = reaction.name, failure, "strict form reaction failed");
                    errors.add(reaction.name.as_str(), "could not be checked");
                }
            }
        }
        errors.into_result()
    }

    /// Runs `reaction` within its time limit; the failure, described.
    async fn run_reaction(
        &self,
        ctx: &Ctx,
        reaction: &InputReaction,
        state: &FormState,
    ) -> Result<Vec<FormPatch>, String> {
        match tokio::time::timeout(reaction.timeout, (reaction.run)(ctx, state)).await {
            Ok(Ok(patches)) => Ok(patches),
            Ok(Err(err)) => Err(err.to_string()),
            Err(_) => Err(format!("took longer than {:?}", reaction.timeout)),
        }
    }

    /// The columns' declared defaults (a create's starting values).
    fn defaults(&self) -> Data {
        self.columns.iter().filter_map(|(name, c)| Some((name.clone(), c.default.clone()?))).collect()
    }

    /// Whether a reaction may see field `name`: readable, not encrypted.
    fn state_field(&self, ctx: &Ctx, name: &str) -> bool {
        self.field(name).is_some_and(|f| f.can_read(ctx) && f.secret.is_none())
    }

    /// The values of `data` a reaction may see.
    fn readable_state(&self, ctx: &Ctx, data: &Data) -> Data {
        data.iter().filter(|(name, _)| self.state_field(ctx, name)).map(|(k, v)| (k.clone(), v.clone())).collect()
    }

    /// Whether the user may write field `name` in `op` (as `accept_input`).
    fn writable(&self, ctx: &Ctx, op: Operation, name: &str) -> bool {
        let natural_create = op == Operation::Create && matches!(self.meta.key, KeyStrategy::Natural);
        self.field(name).is_some_and(|f| f.can_write(op, ctx)) && (name != self.pk_name || natural_create)
    }

    /// The patches the user could make, in order, each applied to a copy of
    /// the state for the next; messages translated. The others are logged
    /// and dropped (the application's bug, not the user's), as are patches
    /// that change nothing.
    fn keep_patches(
        &self,
        ctx: &Ctx,
        op: Operation,
        state: &FormState,
        hidden: &BTreeSet<String>,
        patches: Vec<FormPatch>,
    ) -> Vec<FormPatch> {
        let mut values = state.values.clone();
        let mut kept = Vec::new();
        for patch in patches {
            match self.keep_patch(ctx, op, hidden, &mut values, &patch) {
                Ok(Some(patch)) => kept.push(patch),
                Ok(None) => {}
                Err(reason) => {
                    tracing::warn!(
                        resource = R::SLUG,
                        input = state.changed,
                        field = patch.field().unwrap_or_default(),
                        reason,
                        "form reaction patch dropped"
                    );
                }
            }
        }
        kept
    }

    /// `patch` as sent (`None`: it changes nothing), applied to `values`;
    /// why it is dropped otherwise.
    fn keep_patch(
        &self,
        ctx: &Ctx,
        op: Operation,
        hidden: &BTreeSet<String>,
        values: &mut Data,
        patch: &FormPatch,
    ) -> Result<Option<FormPatch>, &'static str> {
        let Some(field) = patch.field() else {
            let FormPatch::Notice { message } = patch else { unreachable!("only notices have no field") };
            return Ok(Some(FormPatch::notice(ctx.t_in(R::SLUG, message))));
        };
        let input = self.form.inputs().into_iter().find(|i| i.name == field).ok_or("not an input of the form")?;
        if matches!(input.kind, InputType::Secret | InputType::SecretMap | InputType::BelongsToMany)
            || input.repeater.as_deref().is_some_and(|s| s.relationship)
        {
            return Err("an encrypted, link or relationship repeater input");
        }
        // Written into the user's form: a field the user may read and write.
        if !self.writable(ctx, op, field) || !self.state_field(ctx, field) {
            return Err("not readable and writable by the user");
        }
        if hidden.contains(field) {
            return Err("hidden by its conditions");
        }
        let mut errors = ValidationErrors::new();
        let current = values.get(field);
        if let FormPatch::Set { value, .. } | FormPatch::SetIfEmpty { value, .. } = patch {
            let only_empty = matches!(patch, FormPatch::SetIfEmpty { .. });
            if (only_empty && !is_empty(current)) || current == Some(value) {
                return Ok(None);
            }
            let rule = InputRule {
                options: self.input_options(field),
                bounds: self.number_bounds.get(field).copied(),
                ..self.rule(field, false)
            };
            if !check_input(field, &rule, value, &mut errors) {
                return Err("a value its input refuses");
            }
            if let Some(rows) = self.repeaters.get(field)
                && rows.check(field, value, &mut errors).is_none()
            {
                return Err("rows the repeater refuses");
            }
            values.insert(field.to_owned(), value.clone());
            return Ok(Some(patch.clone()));
        }
        // Row patches: JSON repeaters only (relationship rows are left for later).
        let rows = self.repeaters.get(field).ok_or("a row patch on an input that is not a JSON repeater")?;
        let mut list = current.and_then(serde_json::Value::as_array).cloned().unwrap_or_default();
        let len = list.len();
        let at = |index: u32| usize::try_from(index).ok().filter(|i| *i < len).ok_or("a row index out of range");
        let kept = match patch {
            FormPatch::AddRow { index, row, .. } => {
                let index = match index {
                    Some(index) => {
                        usize::try_from(*index).ok().filter(|i| *i <= len).ok_or("a row index out of range")?
                    }
                    None => len,
                };
                if !rows.fits(len + 1) {
                    return Err("more rows than the repeater allows");
                }
                let checked = rows.check_each(field, index, std::slice::from_ref(row), &mut errors);
                if !errors.is_empty() {
                    return Err("a row the repeater refuses");
                }
                list.insert(index, checked.into_iter().next().unwrap_or_default());
                patch.clone()
            }
            FormPatch::RemoveRow { index, .. } => {
                let index = at(*index)?;
                if !rows.fits(len - 1) {
                    return Err("fewer rows than the repeater needs");
                }
                list.remove(index);
                patch.clone()
            }
            FormPatch::MoveRow { from, to, .. } => {
                let (from, to) = (at(*from)?, at(*to)?);
                if from == to {
                    return Ok(None);
                }
                let row = list.remove(from);
                list.insert(to, row);
                patch.clone()
            }
            FormPatch::FlagRow { index, message, .. } => {
                at(*index)?;
                FormPatch::FlagRow { field: field.to_owned(), index: *index, message: ctx.t_in(R::SLUG, message) }
            }
            FormPatch::Set { .. } | FormPatch::SetIfEmpty { .. } | FormPatch::Notice { .. } => {
                unreachable!("handled above")
            }
        };
        values.insert(field.to_owned(), serde_json::Value::Array(list));
        Ok(Some(kept))
    }
}
