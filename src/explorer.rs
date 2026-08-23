use std::{
    collections::BTreeSet,
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use globset::{Glob, GlobSet, GlobSetBuilder};
use unicode_width::UnicodeWidthStr;

use crate::{AppError, Result};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExplorerEntry {
    pub path: PathBuf,
    pub depth: usize,
    pub is_directory: bool,
    pub is_symlink: bool,
    pub expanded: bool,
    pub ignored: bool,
}

#[derive(Clone, Debug)]
pub struct Explorer {
    pub root: PathBuf,
    pub visible: bool,
    pub selected: usize,
    pub viewport_row: usize,
    pub viewport_column: usize,
    pub open_editors_expanded: bool,
    pub open_editors_header_selected: bool,
    pub root_expanded: bool,
    pub root_header_selected: bool,
    expanded: BTreeSet<PathBuf>,
    exclusions: GlobSet,
    entries: Vec<ExplorerEntry>,
}

impl Explorer {
    pub fn new(root: PathBuf, visible: bool, exclusions: &[String]) -> Result<Self> {
        let exclusions = compile_exclusions(exclusions)?;
        let mut explorer = Self {
            root,
            visible,
            selected: 0,
            viewport_row: 0,
            viewport_column: 0,
            open_editors_expanded: true,
            open_editors_header_selected: false,
            root_expanded: true,
            root_header_selected: false,
            expanded: BTreeSet::new(),
            exclusions,
            entries: Vec::new(),
        };
        explorer.refresh()?;
        Ok(explorer)
    }

    pub fn entries(&self) -> &[ExplorerEntry] {
        &self.entries
    }

    pub fn selected_entry(&self) -> Option<&ExplorerEntry> {
        self.entries.get(self.selected)
    }

    pub fn move_selection(&mut self, delta: isize) {
        if self.open_editors_header_selected {
            if delta > 0 {
                self.open_editors_header_selected = false;
                self.root_header_selected = true;
            }
            return;
        }
        if self.root_header_selected {
            if delta > 0 && self.root_expanded && !self.entries.is_empty() {
                self.root_header_selected = false;
                self.selected = 0;
            } else if delta < 0 {
                self.root_header_selected = false;
                self.open_editors_header_selected = true;
            }
            return;
        }
        if delta < 0 && self.selected == 0 {
            self.root_header_selected = true;
            return;
        }
        self.selected = self
            .selected
            .saturating_add_signed(delta)
            .min(self.entries.len().saturating_sub(1));
    }

    pub fn scroll_vertical(&mut self, delta: isize) {
        self.viewport_row = self
            .viewport_row
            .saturating_add_signed(delta)
            .min(self.entries.len().saturating_sub(1));
    }

    pub fn scroll_horizontal(&mut self, delta: isize) {
        self.viewport_column = self
            .viewport_column
            .saturating_add_signed(delta)
            .min(self.maximum_entry_width());
    }

    pub fn toggle_selected(&mut self) -> Result<Option<PathBuf>> {
        if self.open_editors_header_selected {
            self.open_editors_expanded = !self.open_editors_expanded;
            return Ok(None);
        }
        if self.root_header_selected {
            self.root_expanded = !self.root_expanded;
            self.viewport_row = 0;
            return Ok(None);
        }
        let Some(entry) = self.selected_entry().cloned() else {
            return Ok(None);
        };
        if !entry.is_directory || entry.is_symlink {
            return Ok((!entry.is_directory).then_some(entry.path));
        }
        if !self.expanded.remove(&entry.path) {
            self.expanded.insert(entry.path);
        }
        self.refresh()?;
        Ok(None)
    }

    pub fn refresh(&mut self) -> Result<()> {
        let selected_path = self.selected_entry().map(|entry| entry.path.clone());
        let previous_index = self.selected;
        let mut entries = Vec::new();
        self.walk(&self.root, 0, &mut entries)?;
        let ignored = git_ignored_paths(&self.root, &entries);
        for entry in &mut entries {
            entry.ignored = ignored.contains(&entry.path);
        }
        self.entries = entries;
        self.viewport_row = self
            .viewport_row
            .min(self.entries.len().saturating_sub(1));
        self.selected = selected_path
            .and_then(|path| self.entries.iter().position(|entry| entry.path == path))
            .unwrap_or_else(|| previous_index.min(self.entries.len().saturating_sub(1)));
        Ok(())
    }

    fn maximum_entry_width(&self) -> usize {
        self.entries
            .iter()
            .map(|entry| {
                let name = entry.path.file_name().map_or_else(
                    || entry.path.display().to_string(),
                    |name| name.to_string_lossy().into_owned(),
                );
                2usize
                    .saturating_mul(entry.depth.saturating_add(1))
                    .saturating_add(2)
                    .saturating_add(name.width())
            })
            .max()
            .unwrap_or(0)
    }

    pub fn set_exclusions(&mut self, exclusions: &[String]) -> Result<()> {
        self.exclusions = compile_exclusions(exclusions)?;
        self.refresh()
    }

    pub fn is_excluded(&self, path: &Path) -> bool {
        path.strip_prefix(&self.root)
            .is_ok_and(|relative| self.exclusions.is_match(relative))
    }

    fn walk(&self, directory: &Path, depth: usize, output: &mut Vec<ExplorerEntry>) -> Result<()> {
        let read_dir = fs::read_dir(directory).map_err(|error| {
            AppError::io(format!("could not browse {}", directory.display()), error)
        })?;
        let mut children = read_dir
            .filter_map(std::result::Result::ok)
            .filter_map(|entry| {
                (!self.is_excluded(&entry.path())).then_some(entry)
            })
            .collect::<Vec<_>>();
        children.sort_by(|left, right| {
            let left_is_directory = left.file_type().is_ok_and(|file_type| {
                file_type.is_dir() && !file_type.is_symlink()
            });
            let right_is_directory = right.file_type().is_ok_and(|file_type| {
                file_type.is_dir() && !file_type.is_symlink()
            });
            right_is_directory
                .cmp(&left_is_directory)
                .then_with(|| {
                    left.file_name()
                        .to_string_lossy()
                        .to_ascii_lowercase()
                        .cmp(&right.file_name().to_string_lossy().to_ascii_lowercase())
                })
        });
        for child in children {
            let path = child.path();
            let file_type = child.file_type().map_err(|error| {
                AppError::io(format!("could not inspect {}", path.display()), error)
            })?;
            let is_symlink = file_type.is_symlink();
            let is_directory = file_type.is_dir();
            let expanded = is_directory && !is_symlink && self.expanded.contains(&path);
            output.push(ExplorerEntry {
                path: path.clone(),
                depth,
                is_directory,
                is_symlink,
                expanded,
                ignored: false,
            });
            if expanded {
                self.walk(&path, depth + 1, output)?;
            }
        }
        Ok(())
    }
}

fn git_ignored_paths(root: &Path, entries: &[ExplorerEntry]) -> BTreeSet<PathBuf> {
    let paths = entries
        .iter()
        .filter_map(|entry| entry.path.strip_prefix(root).ok())
        .collect::<Vec<_>>();
    if paths.is_empty() {
        return BTreeSet::new();
    }
    let mut child = match Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["check-ignore", "--no-index", "-z", "--stdin"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(_) => return BTreeSet::new(),
    };
    let input = paths.iter().fold(Vec::new(), |mut input, path| {
        input.extend_from_slice(path.to_string_lossy().as_bytes());
        input.push(0);
        input
    });
    let Some(mut stdin) = child.stdin.take() else {
        return BTreeSet::new();
    };
    if stdin.write_all(&input).is_err() {
        return BTreeSet::new();
    }
    drop(stdin);
    let Ok(output) = child.wait_with_output() else {
        return BTreeSet::new();
    };
    if !output.status.success() && output.status.code() != Some(1) {
        return BTreeSet::new();
    }
    output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(|path| root.join(String::from_utf8_lossy(path).as_ref()))
        .collect()
}

pub fn root_for_startup(startup_path: Option<&Path>, current_directory: &Path) -> PathBuf {
    let startup_path = startup_path.map(|path| {
        let path = if path.is_absolute() {
            path.to_owned()
        } else {
            current_directory.join(path)
        };
        fs::canonicalize(&path).unwrap_or(path)
    });
    match startup_path.as_deref() {
        Some(path) if path.is_dir() => path.to_owned(),
        Some(path) => path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or(current_directory)
            .to_owned(),
        None => current_directory.to_owned(),
    }
}

fn compile_exclusions(patterns: &[String]) -> Result<GlobSet> {
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        let glob = Glob::new(pattern).map_err(|error| {
            AppError::Arguments(format!("invalid Explorer exclusion `{pattern}`: {error}"))
        })?;
        builder.add(glob);
    }
    builder
        .build()
        .map_err(|error| AppError::Arguments(format!("invalid Explorer exclusions: {error}")))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;

    #[test]
    fn file_startup_uses_parent_as_root() {
        let root = Path::new("/workspace");
        assert_eq!(
            root_for_startup(Some(Path::new("/workspace/file.rs")), Path::new("/other")),
            root
        );
    }

    #[test]
    fn relative_file_startup_uses_current_directory_as_root() {
        assert_eq!(
            root_for_startup(Some(Path::new("README.md")), Path::new("/workspace")),
            Path::new("/workspace")
        );
    }

    #[test]
    fn relative_directory_startup_resolves_to_its_actual_name() {
        let current_directory = tempdir().unwrap();
        let workspace = current_directory.path().join("workspace");
        fs::create_dir(&workspace).unwrap();

        assert_eq!(
            root_for_startup(Some(Path::new("workspace")), current_directory.path()),
            workspace
        );
    }

    #[test]
    fn dotfiles_are_visible_and_git_is_excluded() {
        let directory = tempdir().unwrap();
        fs::write(directory.path().join(".env"), "x").unwrap();
        fs::create_dir(directory.path().join(".git")).unwrap();
        let explorer = Explorer::new(
            directory.path().to_owned(),
            true,
            &[".git".to_owned(), ".git/**".to_owned()],
        )
        .unwrap();
        assert!(
            explorer
                .entries()
                .iter()
                .any(|entry| entry.path.ends_with(".env"))
        );
        assert!(
            !explorer
                .entries()
                .iter()
                .any(|entry| entry.path.ends_with(".git"))
        );
    }

    #[test]
    fn directories_precede_files_and_each_group_is_case_insensitively_sorted() {
        let directory = tempdir().unwrap();
        fs::create_dir(directory.path().join("zebra")).unwrap();
        fs::create_dir(directory.path().join("Alpha")).unwrap();
        fs::write(directory.path().join("zebra.txt"), "x").unwrap();
        fs::write(directory.path().join("beta.txt"), "x").unwrap();
        let explorer = Explorer::new(directory.path().to_owned(), true, &[]).unwrap();

        let names = explorer
            .entries()
            .iter()
            .map(|entry| entry.path.file_name().unwrap().to_string_lossy().into_owned())
            .collect::<Vec<_>>();

        assert_eq!(names, ["Alpha", "zebra", "beta.txt", "zebra.txt"]);
    }

    #[test]
    fn refresh_marks_git_ignored_files_and_folders() {
        let directory = tempdir().unwrap();
        let root = directory.path();
        Command::new("git")
            .args(["init", "-q", root.to_str().unwrap()])
            .status()
            .unwrap();
        fs::write(root.join(".gitignore"), "ignored-file\nignored-folder/\n").unwrap();
        fs::write(root.join("ignored-file"), "ignored").unwrap();
        fs::create_dir(root.join("ignored-folder")).unwrap();
        fs::write(root.join("regular-file"), "regular").unwrap();

        let explorer = Explorer::new(root.to_owned(), true, &[]).unwrap();

        assert!(explorer
            .entries()
            .iter()
            .any(|entry| entry.path.ends_with("ignored-file") && entry.ignored));
        assert!(explorer
            .entries()
            .iter()
            .any(|entry| entry.path.ends_with("ignored-folder") && entry.ignored));
        assert!(explorer
            .entries()
            .iter()
            .any(|entry| entry.path.ends_with("regular-file") && !entry.ignored));
    }

    #[test]
    fn refresh_recalculates_ignored_entries_after_gitignore_changes() {
        let directory = tempdir().unwrap();
        let root = directory.path();
        Command::new("git")
            .args(["init", "-q", root.to_str().unwrap()])
            .status()
            .unwrap();
        fs::write(root.join("tracked-later"), "text").unwrap();
        Command::new("git")
            .args(["-C", root.to_str().unwrap(), "add", "tracked-later"])
            .status()
            .unwrap();
        let mut explorer = Explorer::new(root.to_owned(), true, &[]).unwrap();
        assert!(explorer
            .entries()
            .iter()
            .any(|entry| entry.path.ends_with("tracked-later") && !entry.ignored));

        fs::write(root.join(".gitignore"), "tracked-later\n").unwrap();
        explorer.refresh().unwrap();

        assert!(explorer
            .entries()
            .iter()
            .any(|entry| entry.path.ends_with("tracked-later") && entry.ignored));
    }

    #[test]
    fn refresh_without_git_marks_no_entries_ignored() {
        let directory = tempdir().unwrap();
        fs::write(directory.path().join("file.txt"), "text").unwrap();

        let explorer = Explorer::new(directory.path().to_owned(), true, &[]).unwrap();

        assert!(explorer.entries().iter().all(|entry| !entry.ignored));
    }

    #[test]
    fn refresh_preserves_selection_by_path_when_an_earlier_entry_is_added() {
        let directory = tempdir().unwrap();
        let selected_path = directory.path().join("middle.txt");
        fs::write(&selected_path, "middle").unwrap();
        fs::write(directory.path().join("z-last.txt"), "last").unwrap();
        let mut explorer = Explorer::new(directory.path().to_owned(), true, &[]).unwrap();
        explorer.selected = explorer
            .entries()
            .iter()
            .position(|entry| entry.path == selected_path)
            .unwrap();

        fs::write(directory.path().join("a-first.txt"), "first").unwrap();
        explorer.refresh().unwrap();

        assert_eq!(explorer.selected_entry().unwrap().path, selected_path);
    }

    #[test]
    fn keyboard_navigation_reaches_and_toggles_explorer_headers() {
        let directory = tempdir().unwrap();
        fs::write(directory.path().join("file.txt"), "text").unwrap();
        let mut explorer = Explorer::new(directory.path().to_owned(), true, &[]).unwrap();
        explorer.move_selection(-1);
        assert!(explorer.root_header_selected);
        explorer.toggle_selected().unwrap();
        assert!(!explorer.root_expanded);
        explorer.move_selection(-1);
        assert!(explorer.open_editors_header_selected);
        explorer.toggle_selected().unwrap();
        assert!(!explorer.open_editors_expanded);
        explorer.move_selection(1);
        assert!(explorer.root_header_selected);
    }

    #[test]
    fn scroll_offsets_are_bounded_to_explorer_content() {
        let directory = tempdir().unwrap();
        fs::write(directory.path().join("long-file-name.txt"), "text").unwrap();
        let mut explorer = Explorer::new(directory.path().to_owned(), true, &[]).unwrap();

        explorer.scroll_vertical(5);
        explorer.scroll_horizontal(5);

        assert_eq!(explorer.viewport_row, 0);
        assert_eq!(explorer.viewport_column, 5);
        explorer.scroll_horizontal(-10);
        assert_eq!(explorer.viewport_column, 0);
    }

    #[cfg(unix)]
    #[test]
    fn directory_symlinks_are_not_expanded() {
        use std::os::unix::fs::symlink;

        let directory = tempdir().unwrap();
        fs::create_dir(directory.path().join("real")).unwrap();
        symlink(
            directory.path().join("real"),
            directory.path().join("linked"),
        )
        .unwrap();
        let mut explorer = Explorer::new(directory.path().to_owned(), true, &[]).unwrap();
        explorer.selected = explorer
            .entries()
            .iter()
            .position(|entry| entry.path.ends_with("linked"))
            .unwrap();
        assert_eq!(
            explorer.toggle_selected().unwrap(),
            Some(directory.path().join("linked"))
        );
        assert!(explorer.entries().iter().all(|entry| entry.depth == 0));
    }
}
