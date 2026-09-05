use std::collections::HashSet;
use std::path::Path;

use wisp_core::fs_plan::preview::{
    SelectedPathKind, SelectedPathPreview, SelectionPreview as CoreSelectionPreview,
    inspect_selected_paths,
};

use crate::error::{AppError, AppResult};
use crate::types::{SelectionChange, SelectionItem, SelectionPreview, SendConfig, SendInput};

use super::destination::SendDestination;
use super::session::SendSession;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendDraft {
    config: SendConfig,
    inputs: Vec<SendInput>,
    /// When set, this is a text-only send: `inputs` is empty and the text
    /// travels inline (≤ 16 KB) or as a synthetic `.txt` (larger).
    inline_text: Option<String>,
}

impl SendDraft {
    pub fn new(config: SendConfig, inputs: Vec<SendInput>) -> Self {
        let mut draft = Self {
            config,
            inputs: Vec::new(),
            inline_text: None,
        };
        draft.replace_inputs(inputs);
        draft
    }

    /// Build a text-only draft.  The text is sent inline when small enough,
    /// otherwise it falls back to a `.txt` file (handled in the core sender).
    pub fn new_text(config: SendConfig, text: String) -> Self {
        Self {
            config,
            inputs: Vec::new(),
            inline_text: Some(text),
        }
    }

    pub fn config(&self) -> &SendConfig {
        &self.config
    }

    pub fn inputs(&self) -> &[SendInput] {
        &self.inputs
    }

    pub fn inline_text(&self) -> Option<&str> {
        self.inline_text.as_deref()
    }

    pub fn replace_inputs(&mut self, inputs: Vec<SendInput>) {
        let mut seen = HashSet::new();
        self.inputs = inputs
            .into_iter()
            .filter(|input| seen.insert(selection_path_key(input.path())))
            .collect();
    }

    pub fn add_inputs(&mut self, inputs: Vec<SendInput>) -> SelectionChange {
        let before = self.inputs.len();
        let mut seen = self
            .inputs
            .iter()
            .map(|input| selection_path_key(input.path()))
            .collect::<HashSet<_>>();

        for input in inputs {
            if seen.insert(selection_path_key(input.path())) {
                self.inputs.push(input);
            }
        }

        let added = self.inputs.len().saturating_sub(before) as u64;
        SelectionChange {
            inputs: self.inputs.clone(),
            added_count: added,
            removed_count: 0,
            changed: added > 0,
        }
    }

    pub fn remove_path(&mut self, path: &Path) -> SelectionChange {
        let key = selection_path_key(path);
        let before = self.inputs.len();
        self.inputs
            .retain(|input| selection_path_key(input.path()) != key);
        let removed = before.saturating_sub(self.inputs.len()) as u64;
        SelectionChange {
            inputs: self.inputs.clone(),
            added_count: 0,
            removed_count: removed,
            changed: removed > 0,
        }
    }

    pub fn clear_inputs(&mut self) {
        self.inputs.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.inputs.is_empty() && self.inline_text.is_none()
    }

    pub fn inspect(&self) -> AppResult<SelectionPreview> {
        if let Some(text) = &self.inline_text {
            // Text-only draft: synthesize a single-item preview so the UI and
            // the connecting/progress events show "1 item · <size>".
            let total_size = text.len() as u64;
            return Ok(SelectionPreview {
                items: vec![SelectionItem {
                    name: "Text snippet".to_owned(),
                    path: "Text snippet".to_owned(),
                    is_directory: false,
                    file_count: 1,
                    total_size,
                }],
                file_count: 1,
                total_size,
            });
        }
        let preview = inspect_selected_paths(&self.inputs).map_err(|e| AppError::Internal {
            message: e.to_string(),
        })?;
        Ok(map_preview(preview))
    }

    pub async fn scan_nearby(
        &self,
        timeout_secs: u64,
    ) -> AppResult<Vec<crate::types::NearbyReceiver>> {
        crate::nearby::scan_nearby_receivers(timeout_secs).await
    }

    pub fn into_session(self, destination: SendDestination) -> SendSession {
        SendSession::new(self, destination)
    }
}

fn map_preview(preview: CoreSelectionPreview) -> SelectionPreview {
    SelectionPreview {
        items: preview.items.into_iter().map(map_item).collect(),
        file_count: preview.file_count,
        total_size: preview.total_size,
    }
}

fn map_item(item: SelectedPathPreview) -> SelectionItem {
    SelectionItem {
        name: item.name,
        path: item.path.display().to_string(),
        is_directory: item.kind == SelectedPathKind::Folder,
        file_count: item.file_count,
        total_size: item.total_size,
    }
}

fn selection_path_key(path: &Path) -> String {
    path.to_string_lossy().trim().to_owned()
}

#[cfg(test)]
mod tests {
    use super::SendDraft;
    use crate::error::{AppError, UserFacingErrorKind};
    use crate::types::{SendConfig, SendInput};
    use std::path::{Path, PathBuf};

    fn inputs(paths: &[&str]) -> Vec<SendInput> {
        paths
            .iter()
            .map(|path| SendInput::from(PathBuf::from(path)))
            .collect()
    }

    fn input_paths(draft: &SendDraft) -> Vec<PathBuf> {
        draft
            .inputs()
            .iter()
            .map(|input| input.path().to_path_buf())
            .collect()
    }
    use wisp_core::protocol::{CancelPhase, TransferRole};
    use wisp_core::transfer::TransferCancellation;

    use super::super::destination::display_destination_label;

    #[test]
    fn destination_label_falls_back_for_unknown_values() {
        assert_eq!(
            display_destination_label("unknown-device"),
            "Recipient device"
        );
        assert_eq!(display_destination_label(""), "Recipient device");
    }

    #[test]
    fn draft_constructor_preserves_order_and_dedupes() {
        let draft = SendDraft::new(
            SendConfig {
                device_name: "Laptop".to_owned(),
                device_type: "laptop".to_owned(),
            },
            inputs(&["a.txt", "b.txt", "a.txt"]),
        );
        assert_eq!(
            input_paths(&draft),
            [PathBuf::from("a.txt"), PathBuf::from("b.txt")]
        );
    }

    #[test]
    fn remove_path_removes_matching_item() {
        let mut draft = SendDraft::new(
            SendConfig {
                device_name: "Laptop".to_owned(),
                device_type: "laptop".to_owned(),
            },
            inputs(&["a.txt", "b.txt"]),
        );

        let change = draft.remove_path(Path::new("a.txt"));

        assert!(change.changed);
        assert_eq!(change.added_count, 0);
        assert_eq!(change.removed_count, 1);
        assert_eq!(input_paths(&draft), [PathBuf::from("b.txt")]);
    }

    #[test]
    fn add_paths_appends_unique_items_only() {
        let mut draft = SendDraft::new(
            SendConfig {
                device_name: "Laptop".to_owned(),
                device_type: "laptop".to_owned(),
            },
            inputs(&["a.txt"]),
        );

        let change = draft.add_inputs(inputs(&["a.txt", "b.txt", "c.txt"]));

        assert!(change.changed);
        assert_eq!(change.added_count, 2);
        assert_eq!(change.removed_count, 0);
        assert_eq!(change.inputs, inputs(&["a.txt", "b.txt", "c.txt"]));
        assert_eq!(draft.inputs(), change.inputs);
    }

    #[test]
    fn clear_paths_empties_selection() {
        let mut draft = SendDraft::new(
            SendConfig {
                device_name: "Laptop".to_owned(),
                device_type: "laptop".to_owned(),
            },
            inputs(&["a.txt"]),
        );

        draft.clear_inputs();

        assert!(draft.is_empty());
        assert!(draft.inputs().is_empty());
    }

    #[test]
    fn failed_event_uses_structured_error() {
        let error = AppError::Internal {
            message: "boom".to_owned(),
        };
        let preview = crate::types::SelectionPreview {
            items: Vec::new(),
            file_count: 0,
            total_size: 0,
        };
        let event = super::super::session::failed_event_from_error(
            "Remote",
            error.into(),
            &preview,
            None,
            None,
        );

        let error = event.error.expect("structured error");
        assert_eq!(error.kind(), UserFacingErrorKind::Internal);
        assert_eq!(error.title(), "Wisp internal error");
        assert!(error.message().contains("boom"));
    }

    #[test]
    fn receiver_waiting_for_decision_cancel_is_treated_as_decline() {
        let cancellation = TransferCancellation {
            by: TransferRole::Receiver,
            phase: CancelPhase::WaitingForDecision,
            reason: "receiver cancelled before approval".to_owned(),
        };

        assert!(crate::send::destination::is_receiver_decline_cancel(
            &cancellation
        ));
        let sender_cancel = TransferCancellation {
            by: TransferRole::Sender,
            phase: CancelPhase::WaitingForDecision,
            reason: "sender cancelled before approval".to_owned(),
        };
        assert!(!crate::send::destination::is_receiver_decline_cancel(
            &sender_cancel
        ));
    }
}
