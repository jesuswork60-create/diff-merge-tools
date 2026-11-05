//! Running the tool the way a user does, through its entry function and the real binary.

mod common;

use std::process::Command;

use common::TempDir;
use diffmerge::cli::run;

struct Run {
    code: i32,
    out: String,
    err: String,
}

fn dmt(args: &[&str], input: &str) -> Run {
    let args: Vec<String> = args.iter().map(|arg| arg.to_string()).collect();
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let code = run(&args, &mut input.as_bytes(), &mut out, &mut err);
    Run { code, out: String::from_utf8(out).unwrap(), err: String::from_utf8(err).unwrap() }
}

#[test]
fn help_and_bad_commands() {
    assert_eq!(dmt(&["--help"], "").code, 0);
    assert!(dmt(&["--help"], "").out.contains("usage: dmt"));
    let none = dmt(&[], "");
    assert_eq!(none.code, 2);
    assert!(none.err.contains("no command"));
    assert_eq!(dmt(&["frobnicate"], "").code, 2);
    assert_eq!(dmt(&["diff", "--nope", "a", "b"], "").code, 2);
    assert_eq!(dmt(&["diff", "only-one"], "").code, 2);
    assert_eq!(dmt(&["diff", "-U", "lots", "a", "b"], "").code, 2);
}

#[test]
fn diff_of_identical_files_is_silent() {
    let dir = TempDir::new();
    dir.write("a", "x\n");
    dir.write("b", "x\n");
    let result = dmt(&["diff", &dir.arg("a"), &dir.arg("b")], "");
    assert_eq!(result.code, 0);
    assert_eq!(result.out, "");
}

#[test]
fn diff_prints_a_patch_and_exits_one() {
    let dir = TempDir::new();
    dir.write("a", "x\ny\n");
    dir.write("b", "x\nz\n");
    let (a, b) = (dir.arg("a"), dir.arg("b"));
    let result = dmt(&["diff", &a, &b], "");
    assert_eq!(result.code, 1);
    assert_eq!(result.out, format!("--- {a}\n+++ {b}\n@@ -1,2 +1,2 @@\n x\n-y\n+z\n"));
}

#[test]
fn diff_reads_standard_input() {
    let dir = TempDir::new();
    dir.write("a", "x\n");
    let result = dmt(&["diff", &dir.arg("a"), "-"], "y\n");
    assert_eq!(result.code, 1);
    assert!(result.out.ends_with("@@ -1 +1 @@\n-x\n+y\n"));
    assert_eq!(dmt(&["diff", "-", "-"], "").code, 2);
}

#[test]
fn diff_options_reach_the_comparison() {
    let dir = TempDir::new();
    dir.write("a", "x  \n");
    dir.write("b", "x\n");
    let (a, b) = (dir.arg("a"), dir.arg("b"));
    assert_eq!(dmt(&["diff", &a, &b], "").code, 1);
    assert_eq!(dmt(&["diff", "-b", &a, &b], "").code, 0);
    assert_eq!(dmt(&["diff", "-w", &a, &b], "").code, 0);
}

#[test]
fn diff_views() {
    let dir = TempDir::new();
    dir.write("a", "the old word\n");
    dir.write("b", "the new word\n");
    let (a, b) = (dir.arg("a"), dir.arg("b"));
    assert!(dmt(&["diff", "--word", &a, &b], "").out.contains("~the [-old-]{+new+} word\n"));
    assert!(dmt(&["diff", "--stat", &a, &b], "").out.contains("1 file changed, 1 insertion(+), 1 deletion(-)"));
}

#[test]
fn diff_of_binary_files_says_so() {
    let dir = TempDir::new();
    std::fs::write(dir.path().join("a"), b"x\0y").unwrap();
    std::fs::write(dir.path().join("b"), b"x\0z").unwrap();
    let result = dmt(&["diff", &dir.arg("a"), &dir.arg("b")], "");
    assert_eq!(result.code, 1);
    assert!(result.out.starts_with("Binary files "));
}

#[test]
fn diff_of_directories_can_be_fed_to_patch() {
    let old = TempDir::new();
    let new = TempDir::new();
    old.write("keep.txt", "a\nb\n");
    new.write("keep.txt", "a\nB\n");
    old.write("gone.txt", "bye\n");
    new.write("sub/new.txt", "hi\n");
    let root_old = old.path().to_str().unwrap();
    let root_new = new.path().to_str().unwrap();
    let diff = dmt(&["diff", "--prefix", root_old, root_new], "");
    assert_eq!(diff.code, 1);
    let patched = dmt(&["patch", "-p", "1", "-d", root_old], &diff.out);
    assert_eq!(patched.code, 0, "{}", patched.out);
    assert_eq!(dmt(&["diff", root_old, root_new], "").code, 0);
}

#[test]
fn patch_reports_offsets_and_rejects() {
    let dir = TempDir::new();
    dir.write("x", "new top\none\ntwo\n");
    dir.write("y", "unrelated\n");
    let root = dir.path().to_str().unwrap();
    let input = "--- x\n+++ x\n@@ -1,2 +1,2 @@\n one\n-two\n+TWO\n--- y\n+++ y\n@@ -1 +1 @@\n-z\n+Z\n";
    let result = dmt(&["patch", "-d", root], input);
    assert_eq!(result.code, 1);
    assert!(result.out.contains("patched x (modify)\n  hunk 1 at line 2 (offset 1, fuzz 0)\n"), "{}", result.out);
    assert!(result.out.contains("hunk 1 rejected: the expected lines were not found"));
    assert_eq!(dir.read("x"), "new top\none\nTWO\n");
    assert!(dir.exists("y.rej"));
}

#[test]
fn patch_can_run_in_reverse() {
    let dir = TempDir::new();
    dir.write("x", "one\nTWO\n");
    let root = dir.path().to_str().unwrap();
    let result = dmt(&["patch", "-R", "-d", root], "--- x\n+++ x\n@@ -1,2 +1,2 @@\n one\n-two\n+TWO\n");
    assert_eq!(result.code, 0);
    assert_eq!(dir.read("x"), "one\ntwo\n");
}

#[test]
fn patch_dry_run_says_would() {
    let dir = TempDir::new();
    dir.write("x", "one\n");
    let root = dir.path().to_str().unwrap();
    let result = dmt(&["patch", "--dry-run", "-d", root], "--- x\n+++ x\n@@ -1 +1 @@\n-one\n+ONE\n");
    assert_eq!(result.code, 0);
    assert!(result.out.starts_with("would patch x"));
    assert_eq!(dir.read("x"), "one\n");
}

#[test]
fn patch_fuzz_and_offset_limits_are_passed_on() {
    let dir = TempDir::new();
    dir.write("x", "pad\npad\nother\ntwo\n");
    let root = dir.path().to_str().unwrap();
    let input = "--- x\n+++ x\n@@ -1,2 +1,2 @@\n one\n-two\n+TWO\n";
    assert_eq!(dmt(&["patch", "--dry-run", "-F", "0", "-d", root], input).code, 1);
    assert_eq!(dmt(&["patch", "--dry-run", "--max-offset", "1", "-d", root], input).code, 1);
    assert_eq!(dmt(&["patch", "--dry-run", "-d", root], input).code, 0);
}

#[test]
fn patch_input_problems() {
    let dir = TempDir::new();
    let root = dir.path().to_str().unwrap();
    let garbled = dmt(&["patch", "-d", root], "--- x\n+++ x\n@@ -1,3 +1 @@\n x\n");
    assert_eq!(garbled.code, 2);
    assert!(garbled.err.contains("line 5"));
    assert_eq!(dmt(&["patch", "-d", root], "no changes here\n").code, 2);
    assert_eq!(dmt(&["patch", "-d", root], "--- ../x\n+++ ../x\n@@ -1 +1 @@\n-a\n+b\n").code, 2);
    assert_eq!(dmt(&["patch", "-d", root, &dir.arg("missing")], "").code, 2);
}

#[test]
fn merge_clean_and_conflicted() {
    let dir = TempDir::new();
    let base = dir.write("base", "a\nb\nc\nd\ne\n");
    let ours = dir.write("ours", "A\nb\nc\nd\ne\n");
    let theirs = dir.write("theirs", "a\nb\nc\nd\nE\n");
    let clash = dir.write("clash", "X\nb\nc\nd\ne\n");
    let path = |p: &std::path::Path| p.to_str().unwrap().to_string();
    let clean = dmt(&["merge", &path(&ours), &path(&base), &path(&theirs)], "");
    assert_eq!(clean.code, 0);
    assert_eq!(clean.out, "A\nb\nc\nd\nE\n");
    let conflict = dmt(&["merge", "-L", "mine", "-L", "orig", "-L", "yours", "--style", "diff3", &path(&ours), &path(&base), &path(&clash)], "");
    assert_eq!(conflict.code, 1);
    assert!(conflict.out.starts_with("<<<<<<< mine\nA\n||||||| orig\na\n=======\nX\n>>>>>>> yours\n"), "{}", conflict.out);
    let settled = dmt(&["merge", "--favor", "theirs", &path(&ours), &path(&base), &path(&clash)], "");
    assert_eq!(settled.code, 0);
    assert_eq!(settled.out, "X\nb\nc\nd\ne\n");
}

#[test]
fn merge_arguments_and_output_file() {
    let dir = TempDir::new();
    let (a, b, c) = (dir.arg("a"), dir.arg("b"), dir.arg("c"));
    dir.write("a", "x\n");
    dir.write("b", "x\n");
    dir.write("c", "y\n");
    let out = dir.arg("out");
    let written = dmt(&["merge", "-o", &out, &a, &b, &c], "");
    assert_eq!(written.code, 0);
    assert_eq!(written.out, "");
    assert_eq!(dir.read("out"), "y\n");
    assert_eq!(dmt(&["merge", &a, &b], "").code, 2);
    assert_eq!(dmt(&["merge", "--style", "odd", &a, &b, &c], "").code, 2);
    assert_eq!(dmt(&["merge", "--favor", "odd", &a, &b, &c], "").code, 2);
    assert_eq!(dmt(&["merge", "-L", "1", "-L", "2", "-L", "3", "-L", "4", &a, &b, &c], "").code, 2);
    std::fs::write(dir.path().join("blob"), b"\0").unwrap();
    assert_eq!(dmt(&["merge", &a, &dir.arg("blob"), &c], "").code, 2);
}

#[test]
fn stat_summarizes_a_patch() {
    let result = dmt(&["stat"], "--- a\n+++ a\n@@ -1,2 +1,2 @@\n k\n-x\n+y\n");
    assert_eq!(result.code, 0);
    assert_eq!(result.out, " a | 2 +-\n 1 file changed, 1 insertion(+), 1 deletion(-)\n");
    assert_eq!(dmt(&["stat"], "--- a\n+++ a\n@@ bad\n").code, 2);
}

#[test]
fn conflicts_and_resolve_commands() {
    let dir = TempDir::new();
    let file = dir.write("f", "t\n<<<<<<< a\nX\n=======\nY\n>>>>>>> b\n");
    let path = file.to_str().unwrap();
    let listed = dmt(&["conflicts", path], "");
    assert_eq!(listed.code, 1);
    assert_eq!(listed.out, format!("{path}:2: ours 1, theirs 1\n"));
    assert_eq!(dmt(&["resolve", "--theirs", path], "").out, "t\nY\n");
    let out = dir.arg("settled");
    assert_eq!(dmt(&["resolve", "--union", "-o", &out, path], "").code, 0);
    assert_eq!(dir.read("settled"), "t\nX\nY\n");
    assert_eq!(dmt(&["conflicts", &out], "").code, 0);
    assert_eq!(dmt(&["resolve", path], "").code, 2);
    assert_eq!(dmt(&["resolve", "--base", path], "").code, 2);
    let broken = dir.write("broken", "<<<<<<< a\n");
    assert_eq!(dmt(&["conflicts", broken.to_str().unwrap()], "").code, 2);
}

#[test]
fn the_real_binary_exits_with_the_same_codes() {
    let dir = TempDir::new();
    dir.write("a", "x\n");
    dir.write("b", "y\n");
    let binary = env!("CARGO_BIN_EXE_dmt");
    let output = Command::new(binary).args(["diff", &dir.arg("a"), &dir.arg("b")]).output().unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stdout).contains("+y"));
    assert_eq!(Command::new(binary).arg("nonsense").output().unwrap().status.code(), Some(2));
}
