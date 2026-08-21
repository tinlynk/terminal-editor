use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

use globset::{Glob, GlobSet, GlobSetBuilder};

use crate::{AppError, Result};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExplorerEntry {
    pub path: PathBuf,
    pub depth: usize,
    pub is_directory: bool,
    pub is_symlink: bool,
    pub expanded: bool,
}

#[derive(Clone, Debug)]
pub struct Explorer {
    pub root: PathBuf,
    pub visible: bool,
    pub selected: usize,
    pub open_editors_expanded: bool,
    pub open_editors_header_selected: bool,
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
            open_editors_expanded: true,
            open_editors_header_selected: false,
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
                self.selected = 0;
            }
            return;
        }
        if delta < 0 && self.selected == 0 {
            self.open_editors_header_selected = true;
            return;
        }
        self.selected = self
            .selected
            .saturating_add_signed(delta)
            .min(self.entries.len().saturating_sub(1));
    }

    pub fn toggle_selected(&mut self) -> Result<Option<PathBuf>> {
        if self.open_editors_header_selected {
            self.open_editors_expanded = !self.open_editors_expanded;
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
        self.entries = entries;
        self.selected = selected_path
            .and_then(|path| self.entries.iter().position(|entry| entry.path == path))
            .unwrap_or_else(|| previous_index.min(self.entries.len().saturating_sub(1)));
        Ok(())
    }

    pub fn set_exclusions(&mut self, exclusions: &[String]) -> Result<()> {
        self.exclusions = compile_exclusions(exclusions)?;
        self.refresh()
    }

    fn walk(&self, directory: &Path, depth: usize, output: &mut Vec<ExplorerEntry>) -> Result<()> {
        let read_dir = fs::read_dir(directory).map_err(|error| {
            AppError::io(format!("could not browse {}", directory.display()), error)
        })?;
        let mut children = read_dir
            .filter_map(std::result::Result::ok)
            .filter_map(|entry| {
                let relative = entry.path().strip_prefix(&self.root).ok()?.to_owned();
                (!self.exclusions.is_match(&relative)).then_some(entry)
            })
            .collect::<Vec<_>>();
        children.sort_by_key(|entry| entry.file_name().to_string_lossy().to_ascii_lowercase());
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
            });
            if expanded {
                self.walk(&path, depth + 1, output)?;
            }
        }
        Ok(())
    }
}

pub fn root_for_startup(startup_path: Option<&Path>, current_directory: &Path) -> PathBuf {
    match startup_path {
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
    fn keyboard_navigation_reaches_and_toggles_open_editors_header() {
        let directory = tempdir().unwrap();
        fs::write(directory.path().join("file.txt"), "text").unwrap();
        let mut explorer = Explorer::new(directory.path().to_owned(), true, &[]).unwrap();
        explorer.move_selection(-1);
        assert!(explorer.open_editors_header_selected);
        explorer.toggle_selected().unwrap();
        assert!(!explorer.open_editors_expanded);
        explorer.move_selection(1);
        assert!(!explorer.open_editors_header_selected);
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
