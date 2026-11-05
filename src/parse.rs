//! Reading unified diffs.
//!
//! Text around the file sections, like a commit message or the extra header lines git
//! writes, is skipped. Inside a hunk the counts in its header are what say where it ends,
//! so a removed line that happens to read `-- foo` can't be mistaken for a file header.

use std::fmt;

use crate::patch::{FilePatch, Hunk, HunkLine, LineKind, Patch};

/// A problem at a particular line of the patch, counting from 1.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub line: usize,
    pub message: String,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "line {}: {}", self.line, self.message)
    }
}

impl std::error::Error for ParseError {}

struct Reader<'a> {
    lines: Vec<&'a [u8]>,
    at: usize,
}

impl<'a> Reader<'a> {
    fn peek(&self) -> Option<&'a [u8]> {
        self.lines.get(self.at).copied()
    }

    fn error<T>(&self, message: impl Into<String>) -> Result<T, ParseError> {
        Err(ParseError { line: self.at + 1, message: message.into() })
    }
}

/// Parses a whole patch.
pub fn parse_patch(input: &[u8]) -> Result<Patch, ParseError> {
    let mut reader = Reader { lines: crate::lines::split_lines(input), at: 0 };
    let mut patch = Patch::default();
    while let Some(line) = reader.peek() {
        if line.starts_with(b"--- ") && reader.lines.get(reader.at + 1).is_some_and(|next| next.starts_with(b"+++ ")) {
            patch.files.push(parse_file(&mut reader)?);
        } else if line.starts_with(b"GIT binary patch") {
            return reader.error("binary patches are not supported");
        } else if line.starts_with(b"Binary files ") {
            patch.notes.push(String::from_utf8_lossy(line).trim_end().to_string());
            reader.at += 1;
        } else if line.starts_with(b"@@ -") {
            return reader.error("hunk without a file header");
        } else {
            reader.at += 1;
        }
    }
    Ok(patch)
}

fn parse_file(reader: &mut Reader<'_>) -> Result<FilePatch, ParseError> {
    let old_path = parse_path(reader, b"--- ")?;
    reader.at += 1;
    let new_path = parse_path(reader, b"+++ ")?;
    reader.at += 1;
    if old_path.is_none() && new_path.is_none() {
        return reader.error("both paths are /dev/null");
    }
    let mut hunks = Vec::new();
    while reader.peek().is_some_and(|line| line.starts_with(b"@@ -")) {
        hunks.push(parse_hunk(reader)?);
    }
    if hunks.is_empty() {
        return reader.error("file section has no hunks");
    }
    Ok(FilePatch { old_path, new_path, hunks })
}

/// Reads a header's path. Anything after a tab is a timestamp, and `/dev/null` means none.
fn parse_path(reader: &Reader<'_>, prefix: &[u8]) -> Result<Option<String>, ParseError> {
    let line = reader.peek().unwrap_or_default();
    let rest = trim_line_end(&line[prefix.len()..]);
    let rest = rest.split(|&byte| byte == b'\t').next().unwrap_or_default();
    let Ok(path) = std::str::from_utf8(rest) else {
        return reader.error("path is not valid UTF-8");
    };
    if path.is_empty() {
        return reader.error("empty path");
    }
    Ok(if path == "/dev/null" { None } else { Some(path.to_string()) })
}

fn trim_line_end(line: &[u8]) -> &[u8] {
    let line = line.strip_suffix(b"\n").unwrap_or(line);
    line.strip_suffix(b"\r").unwrap_or(line)
}

fn parse_hunk(reader: &mut Reader<'_>) -> Result<Hunk, ParseError> {
    let Some(mut hunk) = reader.peek().and_then(parse_hunk_header) else {
        return reader.error("malformed hunk header");
    };
    if (hunk.old_start == 0 && hunk.old_count != 0) || (hunk.new_start == 0 && hunk.new_count != 0) {
        return reader.error("a hunk with lines can't start at line 0");
    }
    reader.at += 1;
    let mut old_left = hunk.old_count;
    let mut new_left = hunk.new_count;
    while old_left > 0 || new_left > 0 {
        let Some(line) = reader.peek() else {
            return reader.error(format!("patch ends inside a hunk, with {old_left} old and {new_left} new lines missing"));
        };
        let (kind, body) = match line.first() {
            Some(b' ') => (LineKind::Context, &line[1..]),
            Some(b'-') => (LineKind::Remove, &line[1..]),
            Some(b'+') => (LineKind::Add, &line[1..]),
            // Some mail programs strip the single space off an empty context line.
            Some(b'\n') => (LineKind::Context, line),
            Some(b'\r') if line == b"\r\n" => (LineKind::Context, line),
            Some(b'\\') => {
                strip_newline(&mut hunk, reader)?;
                reader.at += 1;
                continue;
            }
            _ => return reader.error(format!("expected {old_left} more old and {new_left} more new lines in this hunk")),
        };
        let (uses_old, uses_new) = (kind != LineKind::Add, kind != LineKind::Remove);
        if (uses_old && old_left == 0) || (uses_new && new_left == 0) {
            return reader.error("hunk has more lines than its header says");
        }
        old_left -= usize::from(uses_old);
        new_left -= usize::from(uses_new);
        let mut text = body.to_vec();
        // The last line of a patch file may have lost its newline. That isn't the same as
        // the line having none, which is what the marker is for.
        if text.last() != Some(&b'\n') {
            text.push(b'\n');
        }
        hunk.lines.push(HunkLine { kind, text });
        reader.at += 1;
    }
    if reader.peek().is_some_and(|line| line.first() == Some(&b'\\')) {
        strip_newline(&mut hunk, reader)?;
        reader.at += 1;
    }
    Ok(hunk)
}

fn strip_newline(hunk: &mut Hunk, reader: &Reader<'_>) -> Result<(), ParseError> {
    let Some(last) = hunk.lines.last_mut() else {
        return reader.error("no-newline marker with no line before it");
    };
    if last.text.last() == Some(&b'\n') {
        last.text.pop();
    }
    Ok(())
}

/// Reads `@@ -a,b +c,d @@ section`. A range without a comma has a count of 1.
fn parse_hunk_header(line: &[u8]) -> Option<Hunk> {
    let text = std::str::from_utf8(trim_line_end(line)).ok();
    let lossy;
    let text = match text {
        Some(text) => text,
        None => {
            lossy = String::from_utf8_lossy(trim_line_end(line)).into_owned();
            &lossy
        }
    };
    let rest = text.strip_prefix("@@ -")?;
    let (old_range, rest) = rest.split_once(" +")?;
    let (new_range, rest) = rest.split_once(" @@")?;
    let (old_start, old_count) = parse_range(old_range)?;
    let (new_start, new_count) = parse_range(new_range)?;
    if !rest.is_empty() && !rest.starts_with(' ') {
        return None;
    }
    Some(Hunk {
        old_start,
        old_count,
        new_start,
        new_count,
        section: rest.trim_start().to_string(),
        lines: Vec::new(),
    })
}

fn parse_range(text: &str) -> Option<(usize, usize)> {
    let digits = |part: &str| if !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()) { part.parse().ok() } else { None };
    match text.split_once(',') {
        Some((start, count)) => Some((digits(start)?, digits(count)?)),
        None => Some((digits(text)?, 1)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::patch::format_patch;

    #[test]
    fn parses_a_simple_patch() {
        let patch = parse_patch(b"--- a.txt\n+++ b.txt\n@@ -1,2 +1,2 @@\n one\n-two\n+TWO\n").unwrap();
        assert_eq!(patch.files.len(), 1);
        let file = &patch.files[0];
        assert_eq!(file.old_path.as_deref(), Some("a.txt"));
        assert_eq!(file.new_path.as_deref(), Some("b.txt"));
        let hunk = &file.hunks[0];
        assert_eq!((hunk.old_start, hunk.old_count, hunk.new_start, hunk.new_count), (1, 2, 1, 2));
        assert_eq!(hunk.lines[1].kind, LineKind::Remove);
        assert_eq!(hunk.lines[1].text, b"two\n");
    }

    #[test]
    fn skips_text_around_the_files() {
        let input = b"commit message here\n\ndiff --git a/x b/x\nindex 123..456 100644\n--- a/x\n+++ b/x\n@@ -1 +1 @@\n-a\n+b\n-- \nsignature\n";
        let patch = parse_patch(input).unwrap();
        assert_eq!(patch.files.len(), 1);
        assert_eq!(patch.files[0].hunks[0].lines.len(), 2);
    }

    #[test]
    fn counts_decide_where_a_hunk_ends() {
        let input = b"--- x\n+++ x\n@@ -1,2 +1 @@\n--- not a header\n-+++ nor this\n+z\n";
        let patch = parse_patch(input).unwrap();
        assert_eq!(patch.files.len(), 1);
        assert_eq!(patch.files[0].hunks[0].lines[0].text, b"-- not a header\n");
        assert_eq!(patch.files[0].hunks[0].lines[1].text, b"+++ nor this\n");
    }

    #[test]
    fn reads_several_files_and_hunks() {
        let input = b"--- a\n+++ a\n@@ -1 +1 @@\n-x\n+y\n@@ -5 +5 @@\n-p\n+q\n--- b\n+++ b\n@@ -1 +1 @@\n-m\n+n\n";
        let patch = parse_patch(input).unwrap();
        assert_eq!(patch.files.len(), 2);
        assert_eq!(patch.files[0].hunks.len(), 2);
        assert_eq!(patch.files[1].old_path.as_deref(), Some("b"));
    }

    #[test]
    fn dev_null_means_no_path() {
        let patch = parse_patch(b"--- /dev/null\n+++ new\n@@ -0,0 +1 @@\n+a\n").unwrap();
        assert_eq!(patch.files[0].old_path, None);
    }

    #[test]
    fn timestamps_after_a_tab_are_dropped() {
        let patch = parse_patch(b"--- a.txt\t2020-01-01\n+++ a.txt\t2020-01-02\n@@ -1 +1 @@\n-a\n+b\n").unwrap();
        assert_eq!(patch.files[0].new_path.as_deref(), Some("a.txt"));
    }

    #[test]
    fn the_marker_removes_a_newline() {
        let patch = parse_patch(b"--- a\n+++ a\n@@ -1 +1 @@\n-x\n\\ No newline at end of file\n+x\n").unwrap();
        let lines = &patch.files[0].hunks[0].lines;
        assert_eq!(lines[0].text, b"x");
        assert_eq!(lines[1].text, b"x\n");
    }

    #[test]
    fn the_marker_after_the_last_line_is_read_too() {
        let patch = parse_patch(b"--- a\n+++ a\n@@ -1 +1 @@\n-x\n+y\n\\ No newline at end of file\n").unwrap();
        assert_eq!(patch.files[0].hunks[0].lines[1].text, b"y");
    }

    #[test]
    fn a_bare_empty_line_is_empty_context() {
        let patch = parse_patch(b"--- a\n+++ a\n@@ -1,2 +1,2 @@\n\n-x\n+y\n").unwrap();
        let lines = &patch.files[0].hunks[0].lines;
        assert_eq!(lines[0].kind, LineKind::Context);
        assert_eq!(lines[0].text, b"\n");
    }

    #[test]
    fn a_missing_final_newline_in_the_patch_is_tolerated() {
        let patch = parse_patch(b"--- a\n+++ a\n@@ -1 +1 @@\n-x\n+y").unwrap();
        assert_eq!(patch.files[0].hunks[0].lines[1].text, b"y\n");
    }

    #[test]
    fn crlf_content_is_kept_but_headers_are_trimmed() {
        let patch = parse_patch(b"--- a\r\n+++ a\r\n@@ -1 +1 @@\r\n-x\r\n+y\r\n").unwrap();
        assert_eq!(patch.files[0].new_path.as_deref(), Some("a"));
        assert_eq!(patch.files[0].hunks[0].lines[0].text, b"x\r\n");
    }

    #[test]
    fn keeps_the_section_text() {
        let patch = parse_patch(b"--- a\n+++ a\n@@ -1 +1 @@ fn main() {\n-x\n+y\n").unwrap();
        assert_eq!(patch.files[0].hunks[0].section, "fn main() {");
    }

    #[test]
    fn a_range_without_a_count_means_one() {
        assert_eq!(parse_range("7"), Some((7, 1)));
        assert_eq!(parse_range("7,0"), Some((7, 0)));
        assert_eq!(parse_range("x"), None);
        assert_eq!(parse_range("-1"), None);
        assert_eq!(parse_range("1,"), None);
    }

    #[test]
    fn reports_short_hunks_with_a_line_number() {
        let error = parse_patch(b"--- a\n+++ a\n@@ -1,3 +1,3 @@\n x\n").unwrap_err();
        assert_eq!(error.line, 5);
        assert!(error.message.contains("ends inside a hunk"));
    }

    #[test]
    fn reports_a_wrong_line_in_a_hunk() {
        let error = parse_patch(b"--- a\n+++ a\n@@ -1,2 +1,2 @@\n x\nnonsense\n").unwrap_err();
        assert_eq!(error.line, 5);
    }

    #[test]
    fn reports_too_many_lines_of_one_kind() {
        let error = parse_patch(b"--- a\n+++ a\n@@ -1,2 +1 @@\n-x\n+y\n+z\n").unwrap_err();
        assert!(error.message.contains("more lines than its header"));
    }

    #[test]
    fn rejects_other_bad_input() {
        assert!(parse_patch(b"--- a\n+++ a\n@@ nonsense @@\n").is_err());
        assert!(parse_patch(b"--- a\n+++ a\nno hunks here\n").is_err());
        assert!(parse_patch(b"@@ -1 +1 @@\n-a\n+b\n").is_err());
        assert!(parse_patch(b"--- /dev/null\n+++ /dev/null\n@@ -0,0 +0,0 @@\n").is_err());
        assert!(parse_patch(b"--- a\n+++ a\n@@ -0,1 +1 @@\n-a\n+b\n").is_err());
        assert!(parse_patch(b"GIT binary patch\n").is_err());
    }

    #[test]
    fn a_line_of_dashes_without_a_new_header_is_just_text() {
        let patch = parse_patch(b"--- end of message\nnothing else\n").unwrap();
        assert!(patch.files.is_empty());
    }

    #[test]
    fn binary_file_notes_are_kept() {
        let patch = parse_patch(b"Binary files a and b differ\n").unwrap();
        assert_eq!(patch.notes, vec!["Binary files a and b differ".to_string()]);
    }

    #[test]
    fn formatting_and_parsing_round_trip() {
        let input = b"--- a\n+++ b\n@@ -1,3 +1,2 @@ head\n keep\n-x\n\\ No newline at end of file\n+y\n";
        let wrong = parse_patch(input);
        assert!(wrong.is_err(), "the old count of 3 is not met");
        let input = b"--- a\n+++ b\n@@ -1,2 +1,2 @@ head\n keep\n-x\n\\ No newline at end of file\n+y\n";
        let patch = parse_patch(input).unwrap();
        assert_eq!(format_patch(&patch), input);
    }
}
