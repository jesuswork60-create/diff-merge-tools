//! Patching and comparing real directories.

mod common;

use common::TempDir;
use diffmerge::apply::{HunkStatus, RejectReason};
use diffmerge::diff::DiffOptions;
use diffmerge::parse::parse_patch;
use diffmerge::tree::{diff_trees, TreeOptions};
use diffmerge::workspace::{apply_patch, Action, PatchError, PatchOptions};

fn patch(text: &str) -> diffmerge::patch::Patch {
    parse_patch(text.as_bytes()).unwrap()
}

const CHANGE_X: &str = "--- x.txt\n+++ x.txt\n@@ -1,2 +1,2 @@\n one\n-two\n+TWO\n";

#[test]
fn modifies_a_file() {
    let dir = TempDir::new();
    dir.write("x.txt", "one\ntwo\n");
    let report = apply_patch(dir.path(), &patch(CHANGE_X), &PatchOptions::default()).unwrap();
    assert!(report.is_clean());
    assert!(report.written);
    assert_eq!(report.files[0].action, Action::Modify);
    assert_eq!(dir.read("x.txt"), "one\nTWO\n");
}

#[test]
fn strips_path_components() {
    let dir = TempDir::new();
    dir.write("src/x.txt", "one\ntwo\n");
    let text = "--- a/src/x.txt\n+++ b/src/x.txt\n@@ -1,2 +1,2 @@\n one\n-two\n+TWO\n";
    let options = PatchOptions { strip: 1, ..PatchOptions::default() };
    apply_patch(dir.path(), &patch(text), &options).unwrap();
    assert_eq!(dir.read("src/x.txt"), "one\nTWO\n");
}

#[test]
fn creates_a_file_and_its_directories() {
    let dir = TempDir::new();
    let text = "--- /dev/null\n+++ deep/er/new.txt\n@@ -0,0 +1,2 @@\n+a\n+b\n";
    let report = apply_patch(dir.path(), &patch(text), &PatchOptions::default()).unwrap();
    assert_eq!(report.files[0].action, Action::Create);
    assert_eq!(dir.read("deep/er/new.txt"), "a\nb\n");
}

#[test]
fn refuses_to_create_over_an_existing_file() {
    let dir = TempDir::new();
    dir.write("new.txt", "already\n");
    let text = "--- /dev/null\n+++ new.txt\n@@ -0,0 +1 @@\n+a\n";
    let report = apply_patch(dir.path(), &patch(text), &PatchOptions::default()).unwrap();
    assert!(!report.is_clean());
    assert_eq!(report.files[0].outcomes, vec![HunkStatus::Rejected(RejectReason::FileProblem)]);
    assert_eq!(dir.read("new.txt"), "already\n");
}

#[test]
fn deletes_a_file_that_empties() {
    let dir = TempDir::new();
    dir.write("old.txt", "a\nb\n");
    let text = "--- old.txt\n+++ /dev/null\n@@ -1,2 +0,0 @@\n-a\n-b\n";
    let report = apply_patch(dir.path(), &patch(text), &PatchOptions::default()).unwrap();
    assert!(report.is_clean());
    assert!(!dir.exists("old.txt"));
}

#[test]
fn a_delete_that_leaves_lines_is_a_problem_but_keeps_the_rest() {
    let dir = TempDir::new();
    dir.write("old.txt", "a\nb\nextra\n");
    let text = "--- old.txt\n+++ /dev/null\n@@ -1,2 +0,0 @@\n-a\n-b\n";
    let report = apply_patch(dir.path(), &patch(text), &PatchOptions::default()).unwrap();
    assert!(!report.is_clean());
    assert!(report.files[0].problem.is_some());
    assert_eq!(dir.read("old.txt"), "extra\n");
}

#[test]
fn a_missing_file_is_reported() {
    let dir = TempDir::new();
    let report = apply_patch(dir.path(), &patch(CHANGE_X), &PatchOptions::default()).unwrap();
    assert!(!report.is_clean());
    assert!(report.files[0].problem.as_deref().unwrap().contains("does not exist"));
}

#[test]
fn dry_run_touches_nothing() {
    let dir = TempDir::new();
    dir.write("x.txt", "one\ntwo\n");
    let options = PatchOptions { dry_run: true, write_rejects: true, ..PatchOptions::default() };
    let report = apply_patch(dir.path(), &patch(CHANGE_X), &options).unwrap();
    assert!(report.is_clean());
    assert!(!report.written);
    assert_eq!(dir.read("x.txt"), "one\ntwo\n");
}

#[test]
fn rejected_hunks_go_to_a_rej_file() {
    let dir = TempDir::new();
    dir.write("x.txt", "something else\n");
    let options = PatchOptions { write_rejects: true, ..PatchOptions::default() };
    let report = apply_patch(dir.path(), &patch(CHANGE_X), &options).unwrap();
    assert!(!report.is_clean());
    assert_eq!(dir.read("x.txt.rej"), "--- x.txt\n+++ x.txt\n@@ -1,2 +1,2 @@\n one\n-two\n+TWO\n");
    assert_eq!(dir.read("x.txt"), "something else\n");
}

#[test]
fn a_rej_file_can_be_applied_later() {
    let dir = TempDir::new();
    dir.write("x.txt", "something else\n");
    let options = PatchOptions { write_rejects: true, ..PatchOptions::default() };
    apply_patch(dir.path(), &patch(CHANGE_X), &options).unwrap();
    dir.write("x.txt", "one\ntwo\n");
    let again = patch(&dir.read("x.txt.rej"));
    assert!(apply_patch(dir.path(), &again, &PatchOptions::default()).unwrap().is_clean());
    assert_eq!(dir.read("x.txt"), "one\nTWO\n");
}

#[test]
fn atomic_writes_nothing_when_any_hunk_fails() {
    let dir = TempDir::new();
    dir.write("x.txt", "one\ntwo\n");
    dir.write("y.txt", "something else\n");
    let text = format!("{CHANGE_X}--- y.txt\n+++ y.txt\n@@ -1 +1 @@\n-missing\n+M\n");
    let options = PatchOptions { atomic: true, write_rejects: true, ..PatchOptions::default() };
    let report = apply_patch(dir.path(), &patch(&text), &options).unwrap();
    assert!(!report.written);
    assert_eq!(dir.read("x.txt"), "one\ntwo\n");
    assert!(!dir.exists("y.txt.rej"));
}

#[test]
fn without_atomic_the_good_files_are_written() {
    let dir = TempDir::new();
    dir.write("x.txt", "one\ntwo\n");
    dir.write("y.txt", "something else\n");
    let text = format!("{CHANGE_X}--- y.txt\n+++ y.txt\n@@ -1 +1 @@\n-missing\n+M\n");
    let report = apply_patch(dir.path(), &patch(&text), &PatchOptions::default()).unwrap();
    assert!(report.written);
    assert_eq!(dir.read("x.txt"), "one\nTWO\n");
}

#[test]
fn later_sections_see_earlier_ones() {
    let dir = TempDir::new();
    dir.write("x.txt", "one\ntwo\n");
    let text = format!("{CHANGE_X}--- x.txt\n+++ x.txt\n@@ -2 +2 @@\n-TWO\n+2\n");
    let report = apply_patch(dir.path(), &patch(&text), &PatchOptions::default()).unwrap();
    assert!(report.is_clean());
    assert_eq!(dir.read("x.txt"), "one\n2\n");
}

#[test]
fn a_file_created_earlier_in_the_patch_can_be_changed_later() {
    let dir = TempDir::new();
    let text = "--- /dev/null\n+++ n\n@@ -0,0 +1 @@\n+a\n--- n\n+++ n\n@@ -1 +1 @@\n-a\n+b\n";
    assert!(apply_patch(dir.path(), &patch(text), &PatchOptions::default()).unwrap().is_clean());
    assert_eq!(dir.read("n"), "b\n");
}

#[test]
fn different_names_prefer_the_old_file_when_it_exists() {
    let dir = TempDir::new();
    dir.write("x.orig", "a\n");
    let text = "--- x.orig\n+++ x.new\n@@ -1 +1 @@\n-a\n+b\n";
    let report = apply_patch(dir.path(), &patch(text), &PatchOptions::default()).unwrap();
    assert_eq!(report.files[0].path, "x.orig");
    assert_eq!(dir.read("x.orig"), "b\n");
    assert!(!dir.exists("x.new"));
}

#[test]
fn different_names_use_the_new_file_when_the_old_is_gone() {
    let dir = TempDir::new();
    dir.write("x.new", "a\n");
    let text = "--- x.orig\n+++ x.new\n@@ -1 +1 @@\n-a\n+b\n";
    apply_patch(dir.path(), &patch(text), &PatchOptions::default()).unwrap();
    assert_eq!(dir.read("x.new"), "b\n");
}

#[test]
fn a_path_leaving_the_root_stops_the_run_before_anything_is_written() {
    let dir = TempDir::new();
    dir.write("x.txt", "one\ntwo\n");
    let text = format!("{CHANGE_X}--- ../evil\n+++ ../evil\n@@ -1 +1 @@\n-a\n+b\n");
    let error = apply_patch(dir.path(), &patch(&text), &PatchOptions::default()).unwrap_err();
    assert!(matches!(error, PatchError::BadPath { .. }));
    assert_eq!(dir.read("x.txt"), "one\ntwo\n");
}

#[test]
fn no_temporary_files_are_left_behind() {
    let dir = TempDir::new();
    dir.write("x.txt", "one\ntwo\n");
    apply_patch(dir.path(), &patch(CHANGE_X), &PatchOptions::default()).unwrap();
    let names: Vec<_> = std::fs::read_dir(dir.path()).unwrap().map(|entry| entry.unwrap().file_name()).collect();
    assert_eq!(names.len(), 1);
}

#[cfg(unix)]
#[test]
fn permissions_are_kept() {
    use std::os::unix::fs::PermissionsExt;
    let dir = TempDir::new();
    let path = dir.write("x.txt", "one\ntwo\n");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o750)).unwrap();
    apply_patch(dir.path(), &patch(CHANGE_X), &PatchOptions::default()).unwrap();
    assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o750);
}

fn two_trees() -> (TempDir, TempDir) {
    let old = TempDir::new();
    let new = TempDir::new();
    old.write("same.txt", "same\n");
    new.write("same.txt", "same\n");
    old.write("sub/changed.txt", "a\nb\n");
    new.write("sub/changed.txt", "a\nB\n");
    old.write("gone.txt", "bye\n");
    new.write("added/fresh.txt", "hello\n");
    (old, new)
}

#[test]
fn tree_diff_lists_every_kind_of_change_in_name_order() {
    let (old, new) = two_trees();
    let patch = diff_trees(old.path(), new.path(), &TreeOptions::default()).unwrap();
    let names: Vec<&str> = patch.files.iter().map(|file| file.display_path()).collect();
    assert_eq!(names, vec!["added/fresh.txt", "gone.txt", "sub/changed.txt"]);
    assert_eq!(patch.files[0].old_path, None);
    assert_eq!(patch.files[1].new_path, None);
}

#[test]
fn a_tree_diff_turns_one_tree_into_the_other() {
    let (old, new) = two_trees();
    let found = diff_trees(old.path(), new.path(), &TreeOptions::default()).unwrap();
    assert!(apply_patch(old.path(), &found, &PatchOptions::default()).unwrap().is_clean());
    let left = diff_trees(old.path(), new.path(), &TreeOptions::default()).unwrap();
    assert!(left.files.is_empty());
    assert!(!old.exists("gone.txt"));
    assert_eq!(old.read("added/fresh.txt"), "hello\n");
}

#[test]
fn tree_prefixes_and_excludes() {
    let (old, new) = two_trees();
    let options = TreeOptions {
        diff: DiffOptions::default(),
        old_prefix: "a/".into(),
        new_prefix: "b/".into(),
        exclude: vec!["sub".into(), "gone.txt".into()],
    };
    let patch = diff_trees(old.path(), new.path(), &options).unwrap();
    assert_eq!(patch.files.len(), 1);
    assert_eq!(patch.files[0].old_path, None);
    assert_eq!(patch.files[0].new_path.as_deref(), Some("b/added/fresh.txt"));
}

#[test]
fn binary_and_empty_files_become_notes() {
    let old = TempDir::new();
    let new = TempDir::new();
    std::fs::write(old.path().join("blob"), b"a\0b").unwrap();
    std::fs::write(new.path().join("blob"), b"a\0c").unwrap();
    new.write("empty", "");
    old.write("was-empty", "");
    let patch = diff_trees(old.path(), new.path(), &TreeOptions::default()).unwrap();
    assert!(patch.files.is_empty());
    assert_eq!(
        patch.notes,
        vec!["Binary files blob differ", "Only in the new tree, and empty: empty", "Only in the old tree, and empty: was-empty"]
    );
}

#[test]
fn a_missing_root_is_an_error() {
    let dir = TempDir::new();
    assert!(diff_trees(&dir.path().join("nope"), dir.path(), &TreeOptions::default()).is_err());
}
