use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    hash::{DefaultHasher, Hash, Hasher},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use crate::{
    AppError, Result,
    config::LineEnding,
    document::{Document, DocumentId},
};

static TEMPORARY_FILE_COUNTER: AtomicU64 = AtomicU64::new(1);
const OPEN_WARNING_PROBE_BYTES: usize = 512;
const MAX_RECOVERY_RECORD_BYTES: u64 = 8 * 1024 * 1024;
const RECOVERY_FORMAT_VERSION: u8 = 1;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RecoveryRecord {
    pub format_version: u8,
    pub path: Option<PathBuf>,
    pub revision: u64,
    pub captured_at_unix_seconds: u64,
    pub text: String,
}

pub fn recovery_directory(user_config_path: &Path, workspace_root: &Path) -> Result<PathBuf> {
    let workspace_root = fs::canonicalize(workspace_root).unwrap_or_else(|_| workspace_root.to_owned());
    let mut hasher = DefaultHasher::new();
    workspace_root.hash(&mut hasher);
    user_config_path
        .parent()
        .map(|directory| directory.join("recovery").join(format!("{:016x}", hasher.finish())))
        .ok_or_else(|| AppError::Arguments("user configuration path has no parent".to_owned()))
}

pub fn write_recovery_record(
    directory: &Path,
    document_id: DocumentId,
    document: &Document,
) -> Result<PathBuf> {
    let path = recovery_record_path(directory, document_id, document);
    let record = RecoveryRecord {
        format_version: RECOVERY_FORMAT_VERSION,
        path: document.path.clone(),
        revision: document.revision(),
        captured_at_unix_seconds: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
        text: document.text(),
    };
    let text = toml::to_string(&record).map_err(|error| AppError::Arguments(error.to_string()))?;
    if text.len() as u64 > MAX_RECOVERY_RECORD_BYTES {
        return Err(AppError::Arguments(format!(
            "recovery data for {} exceeds the {} MiB limit",
            document
                .path
                .as_deref()
                .map_or_else(|| "untitled document".to_owned(), |path| path.display().to_string()),
            MAX_RECOVERY_RECORD_BYTES / (1024 * 1024)
        )));
    }
    atomic_write(&path, text.as_bytes())?;
    #[cfg(unix)]
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).map_err(|error| {
        AppError::io(format!("could not protect recovery record {}", path.display()), error)
    })?;
    Ok(path)
}

pub fn remove_recovery_record(directory: &Path, document_id: DocumentId, document: &Document) -> Result<()> {
    let path = recovery_record_path(directory, document_id, document);
    match fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(AppError::io(
            format!("could not remove recovery record {}", path.display()),
            error,
        )),
    }
}

pub fn load_recovery_records(directory: &Path) -> Result<Vec<(PathBuf, RecoveryRecord)>> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(AppError::io(
                format!("could not read recovery directory {}", directory.display()),
                error,
            ));
        }
    };
    let mut records = Vec::new();
    for entry in entries.filter_map(std::result::Result::ok) {
        let path = entry.path();
        if path.extension().is_none_or(|extension| extension != "toml")
            || entry.metadata().map_or(true, |metadata| metadata.len() > MAX_RECOVERY_RECORD_BYTES)
        {
            continue;
        }
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        let Ok(record) = toml::from_str::<RecoveryRecord>(&text) else {
            continue;
        };
        if record.format_version == RECOVERY_FORMAT_VERSION {
            records.push((path, record));
        }
    }
    records.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(records)
}

pub fn remove_recovery_path(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(AppError::io(
            format!("could not remove recovery record {}", path.display()),
            error,
        )),
    }
}

fn recovery_record_path(directory: &Path, document_id: DocumentId, document: &Document) -> PathBuf {
    let identity = document.path.as_ref().map_or_else(
        || format!("untitled-{}", document_id.0),
        |path| path.to_string_lossy().into_owned(),
    );
    let mut hasher = DefaultHasher::new();
    identity.hash(&mut hasher);
    directory.join(format!("{:016x}.toml", hasher.finish()))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OpenFileWarning {
    Archive(&'static str),
    Binary,
}

impl OpenFileWarning {
    pub fn description(self) -> &'static str {
        match self {
            Self::Archive(format) => format,
            Self::Binary => "binary data",
        }
    }
}

pub fn open_file_warning(path: &Path) -> Result<Option<OpenFileWarning>> {
    let mut file = File::open(path)
        .map_err(|error| AppError::io(format!("could not inspect {}", path.display()), error))?;
    let mut bytes = vec![0; OPEN_WARNING_PROBE_BYTES];
    let length = file
        .read(&mut bytes)
        .map_err(|error| AppError::io(format!("could not inspect {}", path.display()), error))?;
    bytes.truncate(length);

    let warning = if bytes.starts_with(b"PK\x03\x04")
        || bytes.starts_with(b"PK\x05\x06")
        || bytes.starts_with(b"PK\x07\x08")
    {
        Some(OpenFileWarning::Archive("a ZIP archive"))
    } else if bytes.starts_with(b"\x1f\x8b") {
        Some(OpenFileWarning::Archive("a gzip archive"))
    } else if bytes.starts_with(b"Rar!\x1a\x07") {
        Some(OpenFileWarning::Archive("a RAR archive"))
    } else if bytes.starts_with(b"7z\xbc\xaf\x27\x1c") {
        Some(OpenFileWarning::Archive("a 7z archive"))
    } else if bytes.get(257..262) == Some(b"ustar") {
        Some(OpenFileWarning::Archive("a tar archive"))
    } else if bytes.contains(&0) {
        Some(OpenFileWarning::Binary)
    } else {
        None
    };
    Ok(warning)
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct FileSnapshot {
    modified: Option<SystemTime>,
    length: u64,
    content_fingerprint: u64,
}

#[derive(Debug)]
pub enum SaveOutcome {
    Saved,
    ExternalChange,
}

#[derive(Debug, Eq, PartialEq)]
pub enum SaveAsOutcome {
    Saved,
    ExistingTarget,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExternalFileState {
    Unchanged,
    Changed,
    Missing,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExternalFileVersion {
    Present {
        modified: Option<SystemTime>,
        length: u64,
        content_fingerprint: u64,
    },
    Missing,
}

#[derive(Debug)]
pub struct DocumentRegistry {
    documents: BTreeMap<DocumentId, Document>,
    canonical_paths: BTreeMap<PathBuf, DocumentId>,
    snapshots: BTreeMap<DocumentId, FileSnapshot>,
    next_id: u64,
    default_line_ending: LineEnding,
    history_limit: usize,
}

impl DocumentRegistry {
    pub fn new(default_line_ending: LineEnding, history_limit: usize) -> Self {
        Self {
            documents: BTreeMap::new(),
            canonical_paths: BTreeMap::new(),
            snapshots: BTreeMap::new(),
            next_id: 1,
            default_line_ending,
            history_limit,
        }
    }

    pub fn new_untitled(&mut self) -> DocumentId {
        let id = self.allocate_id();
        self.documents.insert(
            id,
            Document::new("", None, self.default_line_ending, self.history_limit, "UTF-8"),
        );
        id
    }

    pub fn new_at_path(&mut self, path: PathBuf) -> Result<DocumentId> {
        let id = self.allocate_id();
        self.documents.insert(
            id,
            Document::new(
                "",
                Some(absolute_path(&path)?),
                self.default_line_ending,
                self.history_limit,
                "UTF-8",
            ),
        );
        Ok(id)
    }

    pub fn open(&mut self, path: &Path) -> Result<(DocumentId, bool)> {
        let canonical = fs::canonicalize(path)
            .map_err(|error| AppError::io(format!("could not open {}", path.display()), error))?;
        if canonical.is_dir() {
            return Err(AppError::Arguments(format!(
                "{} is a directory, not a text file",
                path.display()
            )));
        }
        if let Some(id) = self.canonical_paths.get(&canonical) {
            return Ok((*id, false));
        }
        let bytes = fs::read(&canonical).map_err(|error| {
            AppError::io(format!("could not read {}", canonical.display()), error)
        })?;
        let text = String::from_utf8_lossy(&bytes);
        let encoding = if matches!(text, std::borrow::Cow::Borrowed(_)) {
            "UTF-8"
        } else {
            "Binary"
        };
        let id = self.allocate_id();
        let document = Document::new(
            &text,
            Some(canonical.clone()),
            self.default_line_ending,
            self.history_limit,
            encoding,
        );
        self.snapshots.insert(id, snapshot(&canonical)?);
        self.canonical_paths.insert(canonical, id);
        self.documents.insert(id, document);
        Ok((id, true))
    }

    pub fn get(&self, id: DocumentId) -> &Document {
        &self.documents[&id]
    }

    pub fn get_mut(&mut self, id: DocumentId) -> &mut Document {
        self.documents.get_mut(&id).expect("document must exist")
    }

    pub fn iter(&self) -> impl Iterator<Item = (DocumentId, &Document)> {
        self.documents.iter().map(|(id, document)| (*id, document))
    }

    pub fn apply_editor_defaults(&mut self, default_line_ending: LineEnding, history_limit: usize) {
        self.default_line_ending = default_line_ending;
        self.history_limit = history_limit;
        for document in self.documents.values_mut() {
            document.set_history_limit(history_limit);
        }
    }

    pub fn remove(&mut self, id: DocumentId) -> Option<Document> {
        let document = self.documents.remove(&id)?;
        if let Some(path) = &document.path {
            self.canonical_paths.remove(path);
        }
        self.snapshots.remove(&id);
        Some(document)
    }

    pub fn save(&mut self, id: DocumentId, overwrite_external: bool) -> Result<SaveOutcome> {
        let path = self.documents[&id]
            .path
            .clone()
            .ok_or_else(|| AppError::Arguments("untitled document requires Save As".to_owned()))?;
        if !overwrite_external && self.changed_externally(id, &path)? {
            return Ok(SaveOutcome::ExternalChange);
        }
        let text = self.documents[&id].text();
        atomic_write(&path, text.as_bytes())?;
        self.snapshots.insert(id, snapshot(&path)?);
        self.documents
            .get_mut(&id)
            .expect("document must exist")
            .mark_saved();
        Ok(SaveOutcome::Saved)
    }

    pub fn save_as(
        &mut self,
        id: DocumentId,
        path: &Path,
        overwrite_existing: bool,
    ) -> Result<SaveAsOutcome> {
        let absolute = absolute_path(path)?;
        let existing_canonical = fs::canonicalize(&absolute).ok();
        if let Some(owner) = existing_canonical
            .as_ref()
            .and_then(|canonical| self.canonical_paths.get(canonical))
            && *owner != id
        {
            return Err(AppError::Arguments(format!(
                "{} is already open in another document",
                absolute.display()
            )));
        }
        let previous = self.documents[&id].path.clone();
        let targets_current_document = existing_canonical
            .as_ref()
            .is_some_and(|canonical| previous.as_ref() == Some(canonical));
        if existing_canonical.is_some() && !targets_current_document && !overwrite_existing {
            return Ok(SaveAsOutcome::ExistingTarget);
        }
        let text = self.documents[&id].text();
        atomic_write(&absolute, text.as_bytes())?;
        let canonical = fs::canonicalize(&absolute).map_err(|error| {
            AppError::io(format!("could not resolve {}", absolute.display()), error)
        })?;
        if let Some(previous) = previous {
            self.canonical_paths.remove(&previous);
        }
        self.canonical_paths.insert(canonical.clone(), id);
        self.snapshots.insert(id, snapshot(&canonical)?);
        let document = self.documents.get_mut(&id).expect("document must exist");
        document.path = Some(canonical);
        document.mark_saved();
        Ok(SaveAsOutcome::Saved)
    }

    pub fn reload(&mut self, id: DocumentId, discard_changes: bool) -> Result<()> {
        let document = &self.documents[&id];
        if document.is_modified() && !discard_changes {
            return Err(AppError::Arguments(
                "reload requires confirmation because the document is modified".to_owned(),
            ));
        }
        let path = document.path.clone().ok_or_else(|| {
            AppError::Arguments("untitled document cannot be reloaded".to_owned())
        })?;
        let text = fs::read_to_string(&path)
            .map_err(|error| AppError::io(format!("could not reload {}", path.display()), error))?;
        self.documents.insert(
            id,
            Document::new(
                &text,
                Some(path.clone()),
                self.default_line_ending,
                self.history_limit,
                "UTF-8",
            ),
        );
        self.snapshots.insert(id, snapshot(&path)?);
        Ok(())
    }

    pub fn external_file_state(&self, id: DocumentId) -> Result<ExternalFileState> {
        Ok(self.external_file_state_and_version(id)?.0)
    }

    pub fn external_file_state_and_version(
        &self,
        id: DocumentId,
    ) -> Result<(ExternalFileState, Option<ExternalFileVersion>)> {
        let Some(path) = self.documents[&id].path.as_deref() else {
            return Ok((ExternalFileState::Unchanged, None));
        };
        let current = match snapshot(path) {
            Ok(current) => current,
            Err(AppError::Io { source, .. }) if source.kind() == std::io::ErrorKind::NotFound => {
                let state = if self.snapshots.contains_key(&id) {
                    ExternalFileState::Missing
                } else {
                    ExternalFileState::Unchanged
                };
                return Ok((state, Some(ExternalFileVersion::Missing)));
            }
            Err(error) => return Err(error),
        };
        let state = if self
            .snapshots
            .get(&id)
            .is_some_and(|expected| *expected != current)
        {
            ExternalFileState::Changed
        } else {
            ExternalFileState::Unchanged
        };
        Ok((
            state,
            Some(ExternalFileVersion::Present {
                modified: current.modified,
                length: current.length,
                content_fingerprint: current.content_fingerprint,
            }),
        ))
    }

    pub fn accept_external_file_state(&mut self, id: DocumentId) -> Result<()> {
        let Some(path) = self.documents[&id].path.as_deref() else {
            return Ok(());
        };
        match snapshot(path) {
            Ok(snapshot) => {
                self.snapshots.insert(id, snapshot);
            }
            Err(AppError::Io { source, .. }) if source.kind() == std::io::ErrorKind::NotFound => {
                self.snapshots.remove(&id);
            }
            Err(error) => return Err(error),
        }
        Ok(())
    }

    fn changed_externally(&self, id: DocumentId, path: &Path) -> Result<bool> {
        let current = match snapshot(path) {
            Ok(current) => current,
            Err(AppError::Io { source, .. }) if source.kind() == std::io::ErrorKind::NotFound => {
                return Ok(self.snapshots.contains_key(&id));
            }
            Err(error) => return Err(error),
        };
        Ok(self
            .snapshots
            .get(&id)
            .is_some_and(|expected| *expected != current))
    }

    fn allocate_id(&mut self) -> DocumentId {
        let id = DocumentId(self.next_id);
        self.next_id += 1;
        id
    }
}

fn snapshot(path: &Path) -> Result<FileSnapshot> {
    let metadata = fs::metadata(path)
        .map_err(|error| AppError::io(format!("could not inspect {}", path.display()), error))?;
    let bytes = fs::read(path)
        .map_err(|error| AppError::io(format!("could not read {}", path.display()), error))?;
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    Ok(FileSnapshot {
        modified: metadata.modified().ok(),
        length: metadata.len(),
        content_fingerprint: hasher.finish(),
    })
}

fn absolute_path(path: &Path) -> Result<PathBuf> {
    if path.is_absolute() {
        return Ok(path.to_owned());
    }
    std::env::current_dir()
        .map(|directory| directory.join(path))
        .map_err(|error| AppError::io("could not resolve the current directory", error))
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let target = if path.is_symlink() {
        fs::canonicalize(path).map_err(|error| {
            AppError::io(
                format!("could not resolve symlink {}", path.display()),
                error,
            )
        })?
    } else {
        path.to_owned()
    };
    let parent = target.parent().ok_or_else(|| {
        AppError::Arguments(format!("{} has no parent directory", target.display()))
    })?;
    fs::create_dir_all(parent)
        .map_err(|error| AppError::io(format!("could not create {}", parent.display()), error))?;
    let counter = TEMPORARY_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let file_name = target
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("document");
    let temporary = parent.join(format!(
        ".{file_name}.tinlynk-{}-{counter}.tmp",
        std::process::id()
    ));
    let existing_permissions = fs::metadata(&target)
        .ok()
        .map(|metadata| metadata.permissions());
    let write_result = (|| -> std::io::Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        if let Some(permissions) = existing_permissions {
            file.set_permissions(permissions)?;
        }
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&temporary, &target)?;
        File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if let Err(error) = write_result {
        let _ = fs::remove_file(&temporary);
        return Err(AppError::io(
            format!(
                "could not safely replace {}; use Save As to recover",
                target.display()
            ),
            error,
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;

    #[test]
    fn canonical_aliases_reuse_document() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("file.txt");
        fs::write(&path, "text").unwrap();
        let mut registry = DocumentRegistry::new(LineEnding::Lf, 1024);
        let first = registry.open(&path).unwrap();
        let second = registry.open(&directory.path().join("./file.txt")).unwrap();
        assert_eq!(first.0, second.0);
        assert!(first.1);
        assert!(!second.1);
    }

    #[test]
    fn opening_binary_file_succeeds_with_binary_encoding() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("binary.dat");
        fs::write(&path, [0xff, 0xfe, 0x00, 0x41]).unwrap();
        let mut registry = DocumentRegistry::new(LineEnding::Lf, 1024);

        let (id, _) = registry.open(&path).unwrap();

        let document = registry.get(id);
        assert_eq!(document.encoding, "Binary");
        assert_eq!(registry.iter().count(), 1);
    }

    #[test]
    fn external_change_requires_explicit_overwrite() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("file.txt");
        fs::write(&path, "old").unwrap();
        let mut registry = DocumentRegistry::new(LineEnding::Lf, 1024);
        let (id, _) = registry.open(&path).unwrap();
        registry.get_mut(id).insert(3, " edit");
        fs::write(&path, "external and longer").unwrap();
        assert!(matches!(
            registry.save(id, false).unwrap(),
            SaveOutcome::ExternalChange
        ));
        assert_eq!(fs::read_to_string(&path).unwrap(), "external and longer");
    }

    #[test]
    fn equal_length_external_change_with_restored_timestamp_is_detected() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("file.txt");
        fs::write(&path, "first").unwrap();
        let original_modified = fs::metadata(&path).unwrap().modified().unwrap();
        let mut registry = DocumentRegistry::new(LineEnding::Lf, 1024);
        let (id, _) = registry.open(&path).unwrap();
        registry.get_mut(id).insert(5, " edit");
        fs::write(&path, "other").unwrap();
        File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(original_modified))
            .unwrap();
        assert!(matches!(
            registry.save(id, false).unwrap(),
            SaveOutcome::ExternalChange
        ));
        assert_eq!(fs::read_to_string(path).unwrap(), "other");
    }

    #[test]
    fn external_file_state_detects_changes_and_missing_files() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("file.txt");
        fs::write(&path, "first").unwrap();
        let mut registry = DocumentRegistry::new(LineEnding::Lf, 1024);
        let (id, _) = registry.open(&path).unwrap();
        assert_eq!(
            registry.external_file_state(id).unwrap(),
            ExternalFileState::Unchanged
        );

        fs::write(&path, "second").unwrap();
        assert_eq!(
            registry.external_file_state(id).unwrap(),
            ExternalFileState::Changed
        );
        registry.accept_external_file_state(id).unwrap();
        assert_eq!(
            registry.external_file_state(id).unwrap(),
            ExternalFileState::Unchanged
        );

        fs::remove_file(&path).unwrap();
        assert_eq!(
            registry.external_file_state(id).unwrap(),
            ExternalFileState::Missing
        );
    }

    #[test]
    fn recovery_records_round_trip_and_can_be_removed() {
        let directory = tempdir().unwrap();
        let recovery = directory.path().join("recovery");
        let mut registry = DocumentRegistry::new(LineEnding::Lf, 1024);
        let id = registry.new_untitled();
        registry.get_mut(id).insert(0, "unsaved text");

        write_recovery_record(&recovery, id, registry.get(id)).unwrap();
        let records = load_recovery_records(&recovery).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].1.revision, registry.get(id).revision());
        assert!(records[0].1.captured_at_unix_seconds > 0);
        assert_eq!(records[0].1.path, None);
        assert_eq!(records[0].1.text, "unsaved text");

        remove_recovery_record(&recovery, id, registry.get(id)).unwrap();
        assert!(load_recovery_records(&recovery).unwrap().is_empty());
    }

    #[test]
    fn recovery_directories_are_scoped_to_canonical_workspace_roots() {
        let directory = tempdir().unwrap();
        let user_config = directory.path().join("config.toml");
        let first_workspace = directory.path().join("first");
        let second_workspace = directory.path().join("second");
        fs::create_dir_all(&first_workspace).unwrap();
        fs::create_dir_all(&second_workspace).unwrap();

        let first_recovery = recovery_directory(&user_config, &first_workspace).unwrap();
        let second_recovery = recovery_directory(&user_config, &second_workspace).unwrap();

        assert_ne!(first_recovery, second_recovery);
        assert_eq!(
            first_recovery,
            recovery_directory(&user_config, &fs::canonicalize(&first_workspace).unwrap()).unwrap()
        );

        let mut registry = DocumentRegistry::new(LineEnding::Lf, 1024);
        let document_id = registry.new_untitled();
        registry.get_mut(document_id).insert(0, "first workspace changes");
        write_recovery_record(&first_recovery, document_id, registry.get(document_id)).unwrap();

        assert_eq!(load_recovery_records(&first_recovery).unwrap().len(), 1);
        assert!(load_recovery_records(&second_recovery).unwrap().is_empty());
    }

    #[test]
    fn save_as_rejects_a_path_owned_by_another_loaded_document() {
        let directory = tempdir().unwrap();
        let first_path = directory.path().join("first.txt");
        let second_path = directory.path().join("second.txt");
        fs::write(&first_path, "first").unwrap();
        fs::write(&second_path, "second").unwrap();
        let mut registry = DocumentRegistry::new(LineEnding::Lf, 1024);
        let (first, _) = registry.open(&first_path).unwrap();
        registry.open(&second_path).unwrap();
        registry.get_mut(first).insert(5, " edit");
        let error = registry.save_as(first, &second_path, false).unwrap_err();
        assert!(error.to_string().contains("already open"));
        assert_eq!(fs::read_to_string(second_path).unwrap(), "second");
    }

    #[test]
    fn save_as_requires_confirmation_before_overwriting_an_unopened_file() {
        let directory = tempdir().unwrap();
        let target = directory.path().join("existing.txt");
        fs::write(&target, "existing").unwrap();
        let mut registry = DocumentRegistry::new(LineEnding::Lf, 1024);
        let document = registry.new_untitled();
        registry.get_mut(document).insert(0, "replacement");

        assert_eq!(
            registry.save_as(document, &target, false).unwrap(),
            SaveAsOutcome::ExistingTarget
        );
        assert_eq!(fs::read_to_string(&target).unwrap(), "existing");
        assert!(registry.get(document).path.is_none());

        assert_eq!(
            registry.save_as(document, &target, true).unwrap(),
            SaveAsOutcome::Saved
        );
        assert_eq!(fs::read_to_string(target).unwrap(), "replacement");
    }

    #[test]
    fn open_file_warning_detects_archives_binary_data_and_text() {
        let directory = tempdir().unwrap();
        let zip = directory.path().join("archive.zip");
        let binary = directory.path().join("binary.dat");
        let text = directory.path().join("text.txt");
        fs::write(&zip, b"PK\x03\x04payload").unwrap();
        fs::write(&binary, b"text\0data").unwrap();
        fs::write(&text, b"plain text").unwrap();

        assert_eq!(
            open_file_warning(&zip).unwrap(),
            Some(OpenFileWarning::Archive("a ZIP archive"))
        );
        assert_eq!(
            open_file_warning(&binary).unwrap(),
            Some(OpenFileWarning::Binary)
        );
        assert_eq!(open_file_warning(&text).unwrap(), None);
    }

    #[cfg(unix)]
    #[test]
    fn save_preserves_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempdir().unwrap();
        let path = directory.path().join("file.txt");
        fs::write(&path, "old").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
        atomic_write(&path, b"new").unwrap();
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o640
        );
    }

    #[cfg(unix)]
    #[test]
    fn save_updates_symlink_target_without_replacing_link() {
        use std::os::unix::fs::symlink;

        let directory = tempdir().unwrap();
        let target = directory.path().join("target.txt");
        let link = directory.path().join("link.txt");
        fs::write(&target, "old").unwrap();
        symlink(&target, &link).unwrap();
        atomic_write(&link, b"new").unwrap();
        assert!(link.is_symlink());
        assert_eq!(fs::read_to_string(target).unwrap(), "new");
    }
}
