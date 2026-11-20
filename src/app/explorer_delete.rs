use std::time::Instant;

use super::{Application, FocusOwner, Overlay};
use crate::explorer::ExplorerDeleteTarget;

impl Application {
    pub(super) fn open_explorer_delete(&mut self) {
        if self.explorer.root_header_selected || self.explorer.open_editors_header_selected {
            return;
        }
        let Some(entry) = self.explorer.selected_entry() else {
            return;
        };
        let target = match ExplorerDeleteTarget::capture(entry) {
            Ok(target) => target,
            Err(error) => {
                self.overlay = Some(Overlay::ExplorerDeleteError {
                    message: error.to_string(),
                });
                self.focus_owner = FocusOwner::Overlay;
                return;
            }
        };
        let modified_documents = self
            .documents
            .iter()
            .filter(|(_, document)| {
                document.is_modified()
                    && !target.is_symlink
                    && document.path.as_deref().is_some_and(|path| {
                        path == target.path
                            || (target.is_directory && path.starts_with(&target.path))
                    })
            })
            .count();
        self.overlay = Some(Overlay::ConfirmExplorerDelete {
            target,
            modified_documents,
        });
        self.focus_owner = FocusOwner::Overlay;
    }

    pub(super) fn confirm_explorer_delete(&mut self) {
        let Some(Overlay::ConfirmExplorerDelete { target, .. }) = self.overlay.take() else {
            return;
        };
        let deletion = target.delete();
        let pruning = self.explorer.prune_deleted_expansions(&target.path);
        let refresh = self.explorer.refresh();
        self.last_explorer_refresh = Instant::now();

        let mut errors = Vec::new();
        if let Err(error) = deletion {
            errors.push(if target.is_directory {
                format!("{error}. The directory may have been partially deleted.")
            } else {
                error.to_string()
            });
        }
        if let Err(error) = pruning {
            errors.push(format!(
                "Could not update Explorer expansion state: {error}"
            ));
        }
        if let Err(error) = refresh {
            errors.push(format!("Explorer refresh failed: {error}"));
        }
        if errors.is_empty() {
            self.focus_owner = FocusOwner::Explorer;
            self.message = Some(format!("Deleted {}", target.path.display()));
        } else {
            self.overlay = Some(Overlay::ExplorerDeleteError {
                message: errors.join("\n\n"),
            });
            self.focus_owner = FocusOwner::Overlay;
        }
    }
}
