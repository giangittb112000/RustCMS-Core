//! Infolist builders, in the spirit of `filament/infolists`: how a record's
//! view page shows its values (ADR 0039). Entries sit in the form's layouts.

mod entries;
mod infolist;

pub use entries::*;
pub use infolist::*;
