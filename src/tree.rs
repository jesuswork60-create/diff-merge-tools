//! Comparing two directory trees.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::diff::{diff_file, DiffOptions};
use crate::lines::is_binary;
use crate::patch::Patch;

/// Options for walking the trees.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TreeOptions {
    pub diff: DiffOptions,
    /// Added in front of the paths on each side, such as `a/` and `b/`.
    pub old_prefix: String,
    pub new_prefix: String,
    /// File or directory names to leave out, wherever they appear.
    pub exclude: Vec<String>,
}

/// Every regular file under a root, by its relative path with `/` as separator. Symbolic
/// links are not followed and are reported in `notes`.
fn collect(root: &Path, exclude: &[String], notes: &mut Vec<String>) -> io::Result<BTreeMap<String, PathBuf>> {
    let mut files = BTreeMap::new();
    let mut pending = vec![(root.to_path_buf(), String::new())];
    while let Some((directory, relative)) = pending.pop() {
        for entry in fs::read_dir(&directory)? {
            let entry = entry?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                notes.push(format!("Skipped a name that is not valid UTF-8 in {}", directory.display()));
                continue;
            };
            if exclude.iter().any(|excluded| excluded == name) {
                continue;
            }
            let path = if relative.is_empty() { name.to_string() } else { format!("{relative}/{name}") };
            let kind = entry.file_type()?;
            if kind.is_dir() {
                pending.push((entry.path(), path));
            } else if kind.is_file() {
                files.insert(path, entry.path());
            } else {
                notes.push(format!("Skipped {path}: not a regular file"));
            }
        }
    }
    Ok(files)
}

/// Builds a patch that turns one tree into the other. Binary files that differ and files
/// that are empty on the side where they exist can't be written as hunks, so those are
/// mentioned in `notes` rather than left out silently.
pub fn diff_trees(old_root: &Path, new_root: &Path, options: &TreeOptions) -> io::Result<Patch> {
    let mut patch = Patch::default();
    let old_files = collect(old_root, &options.exclude, &mut patch.notes)?;
    let new_files = collect(new_root, &options.exclude, &mut patch.notes)?;
    let names: BTreeSet<&String> = old_files.keys().chain(new_files.keys()).collect();
    for name in names {
        let old_path = old_files.get(name);
        let new_path = new_files.get(name);
        let old_text = old_path.map(fs::read).transpose()?.unwrap_or_default();
        let new_text = new_path.map(fs::read).transpose()?.unwrap_or_default();
        let old_label = old_path.map(|_| format!("{}{name}", options.old_prefix));
        let new_label = new_path.map(|_| format!("{}{name}", options.new_prefix));
        if old_path.is_some() && new_path.is_some() && old_text == new_text {
            continue;
        }
        if is_binary(&old_text) || is_binary(&new_text) {
            patch.notes.push(format!("Binary files {name} differ"));
            continue;
        }
        let file = diff_file(old_label.as_deref(), &old_text, new_label.as_deref(), &new_text, &options.diff);
        if !file.hunks.is_empty() {
            patch.files.push(file);
        } else if old_path.is_none() {
            patch.notes.push(format!("Only in the new tree, and empty: {name}"));
        } else if new_path.is_none() {
            patch.notes.push(format!("Only in the old tree, and empty: {name}"));
        }
    }
    Ok(patch)
}
