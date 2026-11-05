//! Applying a whole patch to a directory.
//!
//! Everything is worked out in memory first and only then written, so a dry run sees the
//! same result as a real one, and an all-or-nothing run can stop before touching anything.

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::apply::{apply_hunks, ApplyOptions, HunkStatus, RejectReason};
use crate::patch::{write_hunk, FilePatch, Hunk, Patch};

/// Options for patching a directory.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PatchOptions {
    /// Leading path components to remove, as with `patch -p`.
    pub strip: usize,
    /// Work everything out and report it, but write nothing.
    pub dry_run: bool,
    /// Write nothing unless every hunk applies.
    pub atomic: bool,
    /// Leave rejected hunks in `<file>.rej`.
    pub write_rejects: bool,
    pub apply: ApplyOptions,
}

/// What a patch asks to do with a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Create,
    Modify,
    Delete,
}

/// The result for one file section of the patch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileReport {
    pub path: String,
    pub action: Action,
    pub outcomes: Vec<HunkStatus>,
    /// Something wrong with the file as a whole, like a file to create that already exists.
    pub problem: Option<String>,
}

impl FileReport {
    pub fn is_clean(&self) -> bool {
        self.problem.is_none() && self.outcomes.iter().all(|status| matches!(status, HunkStatus::Applied { .. }))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PatchReport {
    pub files: Vec<FileReport>,
    /// False for a dry run and for an atomic run that failed.
    pub written: bool,
}

impl PatchReport {
    pub fn is_clean(&self) -> bool {
        self.files.iter().all(FileReport::is_clean)
    }
}

/// Failures that stop the whole run, as opposed to hunks that don't fit.
#[derive(Debug)]
pub enum PatchError {
    /// A path in the patch can't be used.
    BadPath { path: String, reason: String },
    Io { path: PathBuf, source: io::Error },
}

impl fmt::Display for PatchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PatchError::BadPath { path, reason } => write!(f, "{path}: {reason}"),
            PatchError::Io { path, source } => write!(f, "{}: {source}", path.display()),
        }
    }
}

impl std::error::Error for PatchError {}

/// Removes leading components from a patch path and refuses anything that could reach
/// outside the target directory.
pub fn clean_path(path: &str, strip: usize) -> Result<String, PatchError> {
    let bad = |reason: &str| PatchError::BadPath { path: path.to_string(), reason: reason.to_string() };
    let parts: Vec<&str> = path.split('/').collect();
    if strip >= parts.len() {
        return Err(bad("has fewer components than the strip count"));
    }
    let rest = &parts[strip..];
    if rest[0].is_empty() {
        return Err(bad("absolute paths are not allowed"));
    }
    let mut kept = Vec::with_capacity(rest.len());
    for part in rest {
        match *part {
            "" | "." => {}
            ".." => return Err(bad("paths may not go through ..")),
            part if part.contains('\\') => return Err(bad("backslashes are not allowed in paths")),
            part => kept.push(part),
        }
    }
    if kept.is_empty() {
        return Err(bad("nothing is left of the path"));
    }
    Ok(kept.join("/"))
}

/// The action and file to use for a section. Of two different names, the old one wins if
/// it exists, which is what comparing `file.orig` with `file` needs when patching either.
fn target(file: &FilePatch, strip: usize, root: &Path, state: &Overlay) -> Result<(Action, String), PatchError> {
    let old = file.old_path.as_deref().map(|path| clean_path(path, strip)).transpose()?;
    let new = file.new_path.as_deref().map(|path| clean_path(path, strip)).transpose()?;
    match (old, new) {
        (None, Some(new)) => Ok((Action::Create, new)),
        (Some(old), None) => Ok((Action::Delete, old)),
        (Some(old), Some(new)) => {
            let path = if old == new || state.exists(root, &old) { old } else { new };
            Ok((Action::Modify, path))
        }
        (None, None) => Err(PatchError::BadPath { path: "/dev/null".into(), reason: "section has no path".into() }),
    }
}

/// File contents as they will be once the patch is done. Later sections of a patch see
/// what earlier sections did to the same file.
#[derive(Default)]
struct Overlay {
    changes: BTreeMap<String, Option<Vec<u8>>>,
}

impl Overlay {
    fn exists(&self, root: &Path, path: &str) -> bool {
        match self.changes.get(path) {
            Some(change) => change.is_some(),
            None => root.join(path).is_file(),
        }
    }

    fn read(&self, root: &Path, path: &str) -> Result<Vec<u8>, PatchError> {
        if let Some(Some(text)) = self.changes.get(path) {
            return Ok(text.clone());
        }
        let full = root.join(path);
        fs::read(&full).map_err(|source| PatchError::Io { path: full, source })
    }
}

fn rejected_all(file: &FilePatch) -> Vec<HunkStatus> {
    vec![HunkStatus::Rejected(RejectReason::FileProblem); file.hunks.len()]
}

/// Applies a parsed patch under `root`.
pub fn apply_patch(root: &Path, patch: &Patch, options: &PatchOptions) -> Result<PatchReport, PatchError> {
    let mut overlay = Overlay::default();
    let mut report = PatchReport::default();
    let mut rejects: HashMap<String, Vec<u8>> = HashMap::new();

    for file in &patch.files {
        let (action, path) = target(file, options.strip, root, &overlay)?;
        let exists = overlay.exists(root, &path);
        let problem = match action {
            Action::Create if exists => Some("the file to create already exists".to_string()),
            Action::Modify | Action::Delete if !exists => Some("the file does not exist".to_string()),
            _ => None,
        };
        if let Some(problem) = problem {
            collect_rejects(&mut rejects, &path, file, &rejected_all(file));
            report.files.push(FileReport { path, action, outcomes: rejected_all(file), problem: Some(problem) });
            continue;
        }
        let original = if exists { overlay.read(root, &path)? } else { Vec::new() };
        let applied = apply_hunks(&original, &file.hunks, &options.apply);
        collect_rejects(&mut rejects, &path, file, &applied.outcomes);
        let mut problem = None;
        if action == Action::Delete && applied.is_clean() {
            if applied.text.is_empty() {
                overlay.changes.insert(path.clone(), None);
            } else {
                problem = Some("the file was to be deleted but still has lines in it".to_string());
                overlay.changes.insert(path.clone(), Some(applied.text));
            }
        } else if applied.text != original || action == Action::Create {
            overlay.changes.insert(path.clone(), Some(applied.text));
        }
        report.files.push(FileReport { path, action, outcomes: applied.outcomes, problem });
    }

    if options.dry_run || (options.atomic && !report.is_clean()) {
        return Ok(report);
    }
    commit(root, &overlay)?;
    if options.write_rejects {
        for (path, text) in &rejects {
            let full = root.join(format!("{path}.rej"));
            create_parents(&full)?;
            fs::write(&full, text).map_err(|source| PatchError::Io { path: full.clone(), source })?;
        }
    }
    report.written = true;
    Ok(report)
}

fn collect_rejects(rejects: &mut HashMap<String, Vec<u8>>, path: &str, file: &FilePatch, outcomes: &[HunkStatus]) {
    let failed: Vec<&Hunk> = file
        .hunks
        .iter()
        .zip(outcomes)
        .filter(|(_, status)| matches!(status, HunkStatus::Rejected(_)))
        .map(|(hunk, _)| hunk)
        .collect();
    if failed.is_empty() {
        return;
    }
    let text = rejects.entry(path.to_string()).or_default();
    if text.is_empty() {
        text.extend_from_slice(format!("--- {path}\n+++ {path}\n").as_bytes());
    }
    for hunk in failed {
        write_hunk(text, hunk);
    }
}

fn create_parents(path: &Path) -> Result<(), PatchError> {
    match path.parent() {
        Some(parent) => fs::create_dir_all(parent).map_err(|source| PatchError::Io { path: parent.to_path_buf(), source }),
        None => Ok(()),
    }
}

fn commit(root: &Path, overlay: &Overlay) -> Result<(), PatchError> {
    for (path, change) in &overlay.changes {
        let full = root.join(path);
        match change {
            None => fs::remove_file(&full).map_err(|source| PatchError::Io { path: full.clone(), source })?,
            Some(text) => {
                create_parents(&full)?;
                write_replacing(&full, text).map_err(|source| PatchError::Io { path: full.clone(), source })?;
            }
        }
    }
    Ok(())
}

/// Writes next to the target and renames over it, so a failed write can't leave half a
/// file. An existing file's permissions are carried over.
fn write_replacing(path: &Path, text: &[u8]) -> io::Result<()> {
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(".dmt-tmp");
    let temporary = PathBuf::from(temporary);
    fs::write(&temporary, text)?;
    if let Ok(metadata) = fs::metadata(path) {
        fs::set_permissions(&temporary, metadata.permissions())?;
    }
    fs::rename(&temporary, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_components() {
        assert_eq!(clean_path("a/src/x.rs", 1).unwrap(), "src/x.rs");
        assert_eq!(clean_path("src/x.rs", 0).unwrap(), "src/x.rs");
        assert_eq!(clean_path("/abs/x.rs", 2).unwrap(), "x.rs");
    }

    #[test]
    fn tidies_harmless_oddities() {
        assert_eq!(clean_path("./src//x.rs", 0).unwrap(), "src/x.rs");
    }

    #[test]
    fn refuses_paths_that_leave_the_root() {
        assert!(clean_path("../x", 0).is_err());
        assert!(clean_path("a/../../x", 1).is_err());
        assert!(clean_path("a/b/../c", 0).is_err());
        assert!(clean_path("/etc/passwd", 0).is_err());
        assert!(clean_path("a\\..\\b", 0).is_err());
    }

    #[test]
    fn refuses_a_strip_count_that_eats_the_path() {
        assert!(clean_path("a/b", 2).is_err());
        assert!(clean_path("./", 0).is_err());
    }
}
