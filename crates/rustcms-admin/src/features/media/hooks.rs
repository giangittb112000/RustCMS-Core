//! Hooks on the media library: the application's code, run in the
//! transaction of each change to a media record (an upload, a rename or new
//! alternative text, trashing, restoring, deleting for good). An error rolls
//! the change back. Typical use: dispatch a job in that transaction (outbox)
//! that tells a public site to refresh what shows the file.

use super::*;

/// What happened to a media record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum MediaEvent {
    /// A file was uploaded (the row is inserted).
    Uploaded,
    /// Its name or alternative text is about to change: `media` is the record
    /// as it will be stored, `previous` as it is. The row is written after the
    /// hooks, in the same transaction.
    Updated,
    /// Moved to the trash.
    Trashed,
    /// Restored from the trash.
    Restored,
    /// Deleted for good (its file is queued for deletion in the same
    /// transaction).
    Deleted,
}

/// A change to a media record, as hooks receive it.
#[derive(Debug)]
#[non_exhaustive]
pub struct MediaChange<'a> {
    /// What happened.
    pub event: MediaEvent,
    /// The record after the change (for [`MediaEvent::Deleted`], as it was).
    pub media: &'a entity::Model,
    /// The record before the change, for [`MediaEvent::Updated`].
    pub previous: Option<&'a entity::Model>,
}

/// Code run on every change to a media record, registered with
/// [`crate::AdminProvider::media_hook`].
///
/// It runs inside the change's transaction, with the acting user's context:
/// write only through `txn` (or [`rustcms_core::queue::dispatch`] with it),
/// and keep it short (the media row is locked). An error rolls the change
/// back and is answered to the user.
#[async_trait]
pub trait MediaHook: Send + Sync + 'static {
    /// Called once per change.
    async fn changed(&self, ctx: &Ctx, txn: &DatabaseTransaction, change: &MediaChange<'_>) -> AppResult<()>;
}

/// Runs every registered hook, in registration order.
pub(crate) async fn run(ctx: &Ctx, txn: &DatabaseTransaction, change: MediaChange<'_>) -> AppResult<()> {
    for hook in ctx.registry.media_hooks() {
        hook.changed(ctx, txn, &change).await?;
    }
    Ok(())
}
