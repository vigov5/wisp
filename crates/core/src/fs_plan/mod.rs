//! Local filesystem planning for transfers.
//!
//! - [`input`] — what the user picked, as the walk/import layer sees it.
//! - [`preview`] — selection preview derived from the sender import walk.

pub mod conflict;
pub mod error;
pub mod input;
pub mod preview;

#[cfg(test)]
mod test_support;

pub use conflict::ConflictPolicy;
pub use error::FsPlanError;
pub use input::SendInput;
pub use preview::{
    SelectedPathKind, SelectedPathPreview, SelectionPreview, inspect_selected_paths,
};
