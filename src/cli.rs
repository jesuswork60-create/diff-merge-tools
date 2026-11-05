//! The `dmt` command line tool.
//!
//! Exit codes follow `diff` and `patch`: 0 for no differences or a clean result, 1 for
//! differences, conflicts or rejected hunks, and 2 for trouble.

use std::fs;
use std::io::{self, Read, Write};
use std::path::Path;

use crate::apply::HunkStatus;
use crate::conflicts::{list_conflicts, resolve, Resolution};
use crate::diff::{diff_file, DiffOptions};
use crate::lines::{is_binary, Whitespace};
use crate::merge::{merge, ConflictStyle, Favor, MergeOptions};
use crate::parse::parse_patch;
use crate::patch::{write_file_patch, Patch};
use crate::stat::{render, stats, FileStat};
use crate::tree::{diff_trees, TreeOptions};
use crate::word::write_word_hunk;
use crate::workspace::{apply_patch, Action, PatchOptions};

const USAGE: &str = "usage: dmt COMMAND [options]

commands:
  diff [options] OLD NEW       compare two files or directories, - for standard input
       -U N                    lines of context (default 3)
       -b                      ignore white space at the ends of lines
       -w                      ignore all white space
       -p                      show the nearest heading above each hunk
       --word                  show changes word by word
       --stat                  show a summary instead of the changes
       --exclude NAME          with directories, leave out this name (repeatable)
       --prefix                with directories, write paths with a/ and b/
  patch [options] [FILE]       apply a patch, read from standard input without FILE
       -p N                    remove N leading path components
       -d DIR                  work in this directory
       -R                      apply in reverse
       -F N                    context lines that may be ignored (default 2)
       -l                      ignore white space at the ends of lines
       --max-offset N          how far a hunk may move
       --dry-run               report but write nothing
       --atomic                write nothing unless every hunk applies
       --no-rej                don't write .rej files
  merge [options] OURS BASE THEIRS
       --style merge|diff3     how to write conflicts
       --favor ours|theirs|union
                               settle conflicts without markers
       -L LABEL                label for ours, then base, then theirs
       -o FILE                 write the result here, not to standard output
  stat [FILE]                  summarize a patch
  conflicts FILE               list conflict blocks left in a file
  resolve --ours|--theirs|--base|--union [-o OUT] FILE
                               settle every conflict block in a file
";

/// An error with the message for standard error.
enum Failure {
    Usage(String),
    Other(String),
    Output(io::Error),
}

impl From<io::Error> for Failure {
    fn from(error: io::Error) -> Self {
        Failure::Output(error)
    }
}

type Outcome = Result<i32, Failure>;

/// Runs the tool on the given arguments and returns its exit code.
pub fn run(args: &[String], stdin: &mut dyn Read, stdout: &mut dyn Write, stderr: &mut dyn Write) -> i32 {
    let outcome = match args.first().map(String::as_str) {
        None => Err(Failure::Usage("no command given".to_string())),
        Some("-h" | "--help" | "help") => stdout.write_all(USAGE.as_bytes()).map(|()| 0).map_err(Failure::from),
        Some("diff") => run_diff(&args[1..], stdin, stdout),
        Some("patch") => run_patch(&args[1..], stdin, stdout),
        Some("merge") => run_merge(&args[1..], stdout),
        Some("stat") => run_stat(&args[1..], stdin, stdout),
        Some("conflicts") => run_conflicts(&args[1..], stdout),
        Some("resolve") => run_resolve(&args[1..], stdout),
        Some(other) => Err(Failure::Usage(format!("unknown command {other}"))),
    };
    match outcome {
        Ok(code) => code,
        // The reader went away, as with `dmt diff ... | head`. That isn't a failure.
        Err(Failure::Output(error)) if error.kind() == io::ErrorKind::BrokenPipe => 0,
        Err(Failure::Output(error)) => {
            let _ = writeln!(stderr, "dmt: {error}");
            2
        }
        Err(Failure::Usage(message)) => {
            let _ = writeln!(stderr, "dmt: {message}\n\n{USAGE}");
            2
        }
        Err(Failure::Other(message)) => {
            let _ = writeln!(stderr, "dmt: {message}");
            2
        }
    }
}

/// Walks over the arguments of one command.
struct Args<'a> {
    items: std::slice::Iter<'a, String>,
}

impl<'a> Args<'a> {
    fn new(items: &'a [String]) -> Self {
        Args { items: items.iter() }
    }

    fn next(&mut self) -> Option<&'a str> {
        self.items.next().map(String::as_str)
    }

    fn value(&mut self, flag: &str) -> Result<&'a str, Failure> {
        self.next().ok_or_else(|| Failure::Usage(format!("{flag} needs a value")))
    }

    fn number(&mut self, flag: &str) -> Result<usize, Failure> {
        let text = self.value(flag)?;
        text.parse().map_err(|_| Failure::Usage(format!("{flag} needs a number, not {text}")))
    }
}

fn is_flag(arg: &str) -> bool {
    arg.len() > 1 && arg.starts_with('-')
}

fn read_input(path: &str, stdin: &mut dyn Read) -> Result<Vec<u8>, Failure> {
    if path == "-" {
        let mut text = Vec::new();
        stdin.read_to_end(&mut text).map_err(|error| Failure::Other(format!("standard input: {error}")))?;
        Ok(text)
    } else {
        fs::read(path).map_err(|error| Failure::Other(format!("{path}: {error}")))
    }
}

#[derive(Clone, Copy, PartialEq)]
enum DiffView {
    Lines,
    Words,
    Stat,
}

fn run_diff(args: &[String], stdin: &mut dyn Read, stdout: &mut dyn Write) -> Outcome {
    let mut options = DiffOptions::default();
    let mut view = DiffView::Lines;
    let mut exclude = Vec::new();
    let mut prefix = false;
    let mut paths = Vec::new();
    let mut args = Args::new(args);
    while let Some(arg) = args.next() {
        match arg {
            "-U" => options.context = args.number(arg)?,
            "-b" => options.whitespace = Whitespace::IgnoreTrailing,
            "-w" => options.whitespace = Whitespace::IgnoreAll,
            "-p" => options.show_section = true,
            "--word" => view = DiffView::Words,
            "--stat" => view = DiffView::Stat,
            "--exclude" => exclude.push(args.value(arg)?.to_string()),
            "--prefix" => prefix = true,
            flag if is_flag(flag) => return Err(Failure::Usage(format!("unknown option {flag}"))),
            path => paths.push(path),
        }
    }
    let [old, new] = paths[..] else {
        return Err(Failure::Usage("diff needs two paths".to_string()));
    };
    if Path::new(old).is_dir() && Path::new(new).is_dir() {
        let tree = TreeOptions {
            diff: options,
            old_prefix: if prefix { "a/".to_string() } else { String::new() },
            new_prefix: if prefix { "b/".to_string() } else { String::new() },
            exclude,
        };
        let patch = diff_trees(Path::new(old), Path::new(new), &tree).map_err(|error| Failure::Other(error.to_string()))?;
        let differs = !patch.files.is_empty() || !patch.notes.is_empty();
        show_patch(&patch, view, stdout)?;
        return Ok(i32::from(differs));
    }
    if old == "-" && new == "-" {
        return Err(Failure::Usage("only one side can be standard input".to_string()));
    }
    let old_text = read_input(old, stdin)?;
    let new_text = read_input(new, stdin)?;
    if old_text == new_text {
        return Ok(0);
    }
    if is_binary(&old_text) || is_binary(&new_text) {
        writeln!(stdout, "Binary files {old} and {new} differ")?;
        return Ok(1);
    }
    let file = diff_file(Some(old), &old_text, Some(new), &new_text, &options);
    let differs = !file.hunks.is_empty();
    if differs {
        show_patch(&Patch { files: vec![file], notes: Vec::new() }, view, stdout)?;
    }
    Ok(i32::from(differs))
}

fn show_patch(patch: &Patch, view: DiffView, stdout: &mut dyn Write) -> Result<(), Failure> {
    let mut out = Vec::new();
    if view == DiffView::Stat {
        out.extend_from_slice(render(&stats(patch), 40).as_bytes());
    } else {
        for file in &patch.files {
            if view == DiffView::Words {
                let header = crate::patch::FilePatch { hunks: Vec::new(), ..file.clone() };
                write_file_patch(&mut out, &header);
                for hunk in &file.hunks {
                    write_word_hunk(&mut out, hunk);
                }
            } else {
                write_file_patch(&mut out, file);
            }
        }
    }
    for note in &patch.notes {
        out.extend_from_slice(note.as_bytes());
        out.push(b'\n');
    }
    stdout.write_all(&out)?;
    Ok(())
}

fn run_patch(args: &[String], stdin: &mut dyn Read, stdout: &mut dyn Write) -> Outcome {
    let mut options = PatchOptions { write_rejects: true, ..PatchOptions::default() };
    let mut directory = ".".to_string();
    let mut reverse = false;
    let mut input = None;
    let mut args = Args::new(args);
    while let Some(arg) = args.next() {
        match arg {
            "-p" => options.strip = args.number(arg)?,
            "-d" => directory = args.value(arg)?.to_string(),
            "-R" => reverse = true,
            "-F" => options.apply.max_fuzz = args.number(arg)?,
            "-l" => options.apply.whitespace = Whitespace::IgnoreTrailing,
            "--max-offset" => options.apply.max_offset = Some(args.number(arg)?),
            "--dry-run" => options.dry_run = true,
            "--atomic" => options.atomic = true,
            "--no-rej" => options.write_rejects = false,
            flag if is_flag(flag) => return Err(Failure::Usage(format!("unknown option {flag}"))),
            path if input.is_none() => input = Some(path),
            _ => return Err(Failure::Usage("patch takes one input file".to_string())),
        }
    }
    let text = read_input(input.unwrap_or("-"), stdin)?;
    let parsed = parse_patch(&text).map_err(|error| Failure::Other(format!("patch input: {error}")))?;
    if parsed.files.is_empty() {
        return Err(Failure::Other("the input has no changes in it".to_string()));
    }
    let parsed = if reverse { parsed.reversed() } else { parsed };
    let report = apply_patch(Path::new(&directory), &parsed, &options).map_err(|error| Failure::Other(error.to_string()))?;

    let verb = if report.written { "patched" } else { "would patch" };
    for file in &report.files {
        let action = match file.action {
            Action::Create => "create",
            Action::Modify => "modify",
            Action::Delete => "delete",
        };
        writeln!(stdout, "{verb} {} ({action})", file.path)?;
        for (number, status) in file.outcomes.iter().enumerate() {
            match status {
                HunkStatus::Applied { offset: 0, fuzz: 0, .. } => {}
                HunkStatus::Applied { line, offset, fuzz } => {
                    writeln!(stdout, "  hunk {} at line {line} (offset {offset}, fuzz {fuzz})", number + 1)?;
                }
                HunkStatus::Rejected(reason) => writeln!(stdout, "  hunk {} rejected: {}", number + 1, reason.describe())?,
            }
        }
        if let Some(problem) = &file.problem {
            writeln!(stdout, "  {problem}")?;
        }
    }
    for note in &parsed.notes {
        writeln!(stdout, "skipped: {note}")?;
    }
    Ok(i32::from(!report.is_clean()))
}

fn run_merge(args: &[String], stdout: &mut dyn Write) -> Outcome {
    let mut options = MergeOptions::default();
    let mut labels = Vec::new();
    let mut output = None;
    let mut paths = Vec::new();
    let mut args = Args::new(args);
    while let Some(arg) = args.next() {
        match arg {
            "--style" => {
                options.style = match args.value(arg)? {
                    "merge" => ConflictStyle::Merge,
                    "diff3" => ConflictStyle::Diff3,
                    other => return Err(Failure::Usage(format!("unknown style {other}"))),
                };
            }
            "--favor" => {
                options.favor = match args.value(arg)? {
                    "ours" => Favor::Ours,
                    "theirs" => Favor::Theirs,
                    "union" => Favor::Union,
                    other => return Err(Failure::Usage(format!("unknown choice {other}"))),
                };
            }
            "-L" => labels.push(args.value(arg)?.to_string()),
            "-o" => output = Some(args.value(arg)?),
            flag if is_flag(flag) => return Err(Failure::Usage(format!("unknown option {flag}"))),
            path => paths.push(path),
        }
    }
    let [ours, base, theirs] = paths[..] else {
        return Err(Failure::Usage("merge needs three files: OURS BASE THEIRS".to_string()));
    };
    if labels.len() > 3 {
        return Err(Failure::Usage("-L can be given at most three times".to_string()));
    }
    options.labels.ours = labels.first().cloned().unwrap_or_else(|| ours.to_string());
    options.labels.base = labels.get(1).cloned().unwrap_or_else(|| base.to_string());
    options.labels.theirs = labels.get(2).cloned().unwrap_or_else(|| theirs.to_string());
    let read = |path: &str| fs::read(path).map_err(|error| Failure::Other(format!("{path}: {error}")));
    let (ours_text, base_text, theirs_text) = (read(ours)?, read(base)?, read(theirs)?);
    if is_binary(&ours_text) || is_binary(&base_text) || is_binary(&theirs_text) {
        return Err(Failure::Other("can't merge binary files".to_string()));
    }
    let result = merge(&base_text, &ours_text, &theirs_text, &options);
    write_result(output, &result.text, stdout)?;
    Ok(i32::from(!result.is_clean()))
}

fn write_result(output: Option<&str>, text: &[u8], stdout: &mut dyn Write) -> Result<(), Failure> {
    match output {
        Some(path) => fs::write(path, text).map_err(|error| Failure::Other(format!("{path}: {error}"))),
        None => stdout.write_all(text).map_err(Failure::from),
    }
}

fn run_stat(args: &[String], stdin: &mut dyn Read, stdout: &mut dyn Write) -> Outcome {
    let input = match args {
        [] => "-",
        [path] if !is_flag(path) => path.as_str(),
        _ => return Err(Failure::Usage("stat takes one input file".to_string())),
    };
    let text = read_input(input, stdin)?;
    let parsed = parse_patch(&text).map_err(|error| Failure::Other(format!("patch input: {error}")))?;
    let rows: Vec<FileStat> = stats(&parsed);
    stdout.write_all(render(&rows, 40).as_bytes())?;
    Ok(0)
}

fn run_conflicts(args: &[String], stdout: &mut dyn Write) -> Outcome {
    let [path] = args else {
        return Err(Failure::Usage("conflicts takes one file".to_string()));
    };
    let text = fs::read(path).map_err(|error| Failure::Other(format!("{path}: {error}")))?;
    let blocks = list_conflicts(&text).map_err(|error| Failure::Other(format!("{path}: {error}")))?;
    for block in &blocks {
        let base = block.base.as_ref().map_or(String::new(), |lines| format!(", base {}", lines.len()));
        writeln!(stdout, "{path}:{}: ours {}{base}, theirs {}", block.line, block.ours.len(), block.theirs.len())?;
    }
    Ok(i32::from(!blocks.is_empty()))
}

fn run_resolve(args: &[String], stdout: &mut dyn Write) -> Outcome {
    let mut choice = None;
    let mut output = None;
    let mut input = None;
    let mut args = Args::new(args);
    while let Some(arg) = args.next() {
        match arg {
            "--ours" => choice = Some(Resolution::Ours),
            "--theirs" => choice = Some(Resolution::Theirs),
            "--base" => choice = Some(Resolution::Base),
            "--union" => choice = Some(Resolution::Union),
            "-o" => output = Some(args.value(arg)?),
            flag if is_flag(flag) => return Err(Failure::Usage(format!("unknown option {flag}"))),
            path if input.is_none() => input = Some(path),
            _ => return Err(Failure::Usage("resolve takes one file".to_string())),
        }
    }
    let (Some(choice), Some(path)) = (choice, input) else {
        return Err(Failure::Usage("resolve needs a side and a file".to_string()));
    };
    let text = fs::read(path).map_err(|error| Failure::Other(format!("{path}: {error}")))?;
    let settled = resolve(&text, choice).map_err(|error| Failure::Other(format!("{path}: {error}")))?;
    write_result(output, &settled, stdout)?;
    Ok(0)
}
