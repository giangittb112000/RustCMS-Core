//! Code run on every change to a media record (`AdminProvider::media_hook`).

use rustcms::admin::Ctx;
use rustcms::admin::media::{MediaChange, MediaEvent, MediaHook};
use rustcms::{AppResult, ValidationErrors, async_trait};
use sea_orm::DatabaseTransaction;

// #region media-hook
/// The demo's rule for alternative text: it describes the image, so it may not
/// just repeat the file's name. An error rolls the change back. A site that
/// shows media publicly would also dispatch a job here
/// (`rustcms::queue::dispatch(txn, &job)`), to refresh the pages showing it:
/// the job exists only if the change commits.
pub struct MediaChanges;

#[async_trait]
impl MediaHook for MediaChanges {
    async fn changed(&self, _ctx: &Ctx, _txn: &DatabaseTransaction, change: &MediaChange<'_>) -> AppResult<()> {
        let media = change.media;
        if change.event == MediaEvent::Updated && media.alt.as_deref() == Some(media.name.as_str()) {
            return Err(ValidationErrors::field("alt", "describes the image, not its file name").into());
        }
        Ok(())
    }
}
// #endregion media-hook
