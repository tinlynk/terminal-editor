use std::{fs, path::Path};

use crate::{
    Result,
    document::DocumentId,
    file_io::{SaveAsOutcome, SaveOutcome, open_file_warning, remove_recovery_record_for_path},
    layout::SplitDirection,
    workspace::Pane,
};

use super::{Application, FocusOwner, Overlay, file_dialog};

pub(super) fn request_close(application: &mut Application) -> Result<()> {
    if application.focus_owner == FocusOwner::BottomTerminal {
        if application.active_bottom_terminal().is_some() {
            application.overlay = Some(Overlay::ConfirmTerminalClose);
            application.focus_owner = FocusOwner::Overlay;
        }
        return Ok(());
    }
    match application.workspace.focused_pane() {
        Pane::Empty => {
            application.workspace.close_focused();
        }
        Pane::Editor { document_id, .. } => {
            let document_id = *document_id;
            let visible_views = application
                .workspace
                .panes
                .values()
                .filter(|pane| {
                    matches!(pane, Pane::Editor { document_id: id, .. } if *id == document_id)
                })
                .count();
            if visible_views == 1 && application.documents.get(document_id).is_modified() {
                application.overlay = Some(Overlay::ConfirmDocumentClose);
                application.focus_owner = FocusOwner::Overlay;
            } else {
                application.close_focused_editor_view();
                if application.quit_in_progress {
                    application.continue_quit_sequence()?;
                }
            }
        }
    }
    Ok(())
}

pub(super) fn close_focused_editor_view(application: &mut Application) {
    let Some((document_id, pane_id)) = application.focused_editor() else {
        return;
    };
    let next_document = application
        .documents
        .iter()
        .find_map(|(id, _)| (id != document_id).then_some(id));
    if let Some(next_document) = next_document {
        application.workspace.panes.insert(pane_id, Pane::Empty);
        application.unload_document_without_views(document_id);
        application.switch_editor_document(pane_id, next_document);
        return;
    }
    if application.workspace.panes.len() == 1 {
        application.workspace.panes.insert(pane_id, Pane::Empty);
        application.unload_document_without_views(document_id);
    } else {
        application.workspace.close_focused();
        application.unload_document_without_views(document_id);
    }
}

pub(super) fn save_focused(application: &mut Application) -> Result<()> {
    let Some((document_id, _)) = application.focused_editor() else {
        return Ok(());
    };
    if application.documents.get(document_id).path.is_none() {
        application.overlay = Some(file_dialog(
            &application.explorer.root,
            true,
            &application.explorer,
        ));
        application.focus_owner = FocusOwner::Overlay;
        return Ok(());
    }
    let outcome = match application.documents.save(document_id, false) {
        Ok(outcome) => outcome,
        Err(error) => {
            application.message = Some(format!("Could not save document: {error}"));
            return Ok(());
        }
    };
    match outcome {
        SaveOutcome::Saved => application.finish_successful_save(document_id)?,
        SaveOutcome::ExternalChange => {
            application.overlay = Some(Overlay::SaveConflict);
            application.focus_owner = FocusOwner::Overlay;
        }
    }
    Ok(())
}

pub(super) fn save_document_as(
    application: &mut Application,
    document_id: DocumentId,
    path: &Path,
) -> Result<()> {
    let previous_path = application.documents.get(document_id).path.clone();
    let outcome = match application.documents.save_as(document_id, path, false) {
        Ok(outcome) => outcome,
        Err(error) => {
            application.message = Some(format!("Could not save document: {error}"));
            return Ok(());
        }
    };
    match outcome {
        SaveAsOutcome::Saved => {
            if let Some(previous_path) = previous_path
                && let Err(error) = remove_recovery_record_for_path(
                    &application.recovery_directory,
                    document_id,
                    Some(&previous_path),
                )
            {
                application.message = Some(format!("Could not remove old recovery data: {error}"));
            }
            application.finish_successful_save(document_id)
        }
        SaveAsOutcome::ExistingTarget => {
            application.overlay = Some(Overlay::ConfirmSaveAsOverwrite {
                document_id,
                path: path.to_owned(),
            });
            application.focus_owner = FocusOwner::Overlay;
            Ok(())
        }
    }
}

pub(super) fn finish_successful_save(
    application: &mut Application,
    document_id: DocumentId,
) -> Result<()> {
    if let Err(error) = application.remove_recovery_for_document(document_id) {
        application.message = Some(format!("Could not remove saved recovery data: {error}"));
    }
    application.deferred_external_changes.remove(&document_id);
    let saved_path = application.documents.get(document_id).path.clone();
    if saved_path.as_deref().is_some_and(|path| {
        path == application.config.user_path.as_path()
            || path == application.config.workspace_path.as_path()
    }) {
        application.reload_config_after_raw_save()?;
    }
    let theme_directory = fs::canonicalize(application.theme_registry.directory())
        .unwrap_or_else(|_| application.theme_registry.directory().to_owned());
    if saved_path.as_deref().is_some_and(|path| {
        path.parent() == Some(theme_directory.as_path())
            && path
                .extension()
                .is_some_and(|extension| extension == "toml")
    }) {
        application.rescan_themes(true)?;
    }
    if application.pending_close_document == Some(document_id) {
        application.pending_close_document = None;
        if let Some((pane_id, _)) = application.workspace.panes.iter().find(|(_, pane)| {
            matches!(pane, Pane::Editor { document_id: open_id, .. } if *open_id == document_id)
        }) {
            application.workspace.focused = *pane_id;
            application.close_focused_editor_view();
        }
        application.overlay = None;
        application.focus_owner = FocusOwner::Workspace;
        if application.quit_in_progress {
            application.continue_quit_sequence()?;
        }
    }
    Ok(())
}

pub(super) fn open_path(
    application: &mut Application,
    path: &Path,
    split: Option<SplitDirection>,
) -> Result<()> {
    if let Some(warning) = open_file_warning(path)? {
        application.overlay = Some(Overlay::ConfirmUnsafeOpen {
            path: path.to_owned(),
            split,
            warning: warning.description(),
        });
        application.focus_owner = FocusOwner::Overlay;
        return Ok(());
    }
    open_confirmed_path(application, path, split)
}

pub(super) fn open_confirmed_path(
    application: &mut Application,
    path: &Path,
    split: Option<SplitDirection>,
) -> Result<()> {
    let (document_id, _) = application.documents.open(path)?;
    if let Some(direction) = split {
        application
            .workspace
            .split_with_editor(document_id, direction);
        return Ok(());
    }
    if let Some((pane_id, _)) = application.workspace.panes.iter().find(|(_, pane)| {
        matches!(pane, Pane::Editor { document_id: open_id, .. } if *open_id == document_id)
    }) {
        application.workspace.focused = *pane_id;
    } else {
        let focused = application.workspace.focused;
        let replaced_document = match application.workspace.panes.get(&focused) {
            Some(Pane::Editor { document_id, .. }) => Some(*document_id),
            _ => None,
        };
        application.switch_editor_document(focused, document_id);
        if let Some(replaced_document) = replaced_document {
            application.unload_pristine_untitled_without_views(replaced_document);
        }
    }
    Ok(())
}
