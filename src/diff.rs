//! Comparing two texts line by line and grouping the changes into hunks.

use std::collections::HashMap;

use crate::lines::{normalize, split_lines, Whitespace};
use crate::myers::{self, Op, Span};
use crate::patch::{FilePatch, Hunk, HunkLine, LineKind};

/// Choices that affect how a diff comes out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiffOptions {
    /// Unchanged lines to keep around each change.
    pub context: usize,
    pub whitespace: Whitespace,
    /// Show the nearest line above each hunk that looks like a heading or function.
    pub show_section: bool,
}

impl Default for DiffOptions {
    fn default() -> Self {
        DiffOptions { context: 3, whitespace: Whitespace::Exact, show_section: false }
    }
}

/// Compares two sets of lines by number, which makes the comparison in the diff cheap.
fn intern_lines(old: &[&[u8]], new: &[&[u8]], mode: Whitespace) -> (Vec<u32>, Vec<u32>) {
    let mut table = HashMap::new();
    let mut number = |lines: &[&[u8]]| -> Vec<u32> {
        lines
            .iter()
            .map(|line| {
                let next = table.len() as u32;
                *table.entry(normalize(line, mode).into_owned()).or_insert(next)
            })
            .collect()
    };
    let first = number(old);
    let second = number(new);
    (first, second)
}

/// Edit script between two already split texts.
pub fn line_spans(old: &[&[u8]], new: &[&[u8]], mode: Whitespace) -> Vec<Span> {
    let (old_ids, new_ids) = intern_lines(old, new, mode);
    myers::diff(&old_ids, &new_ids)
}

/// Compares two texts and returns the hunks; identical texts give none.
pub fn diff_texts(old: &[u8], new: &[u8], options: &DiffOptions) -> Vec<Hunk> {
    let old_lines = split_lines(old);
    let new_lines = split_lines(new);
    let spans = line_spans(&old_lines, &new_lines, options.whitespace);
    build_hunks(&old_lines, &new_lines, &spans, options)
}

/// The same, wrapped as the patch for a named pair of files.
pub fn diff_file(old_path: Option<&str>, old: &[u8], new_path: Option<&str>, new: &[u8], options: &DiffOptions) -> FilePatch {
    FilePatch {
        old_path: old_path.map(String::from),
        new_path: new_path.map(String::from),
        hunks: diff_texts(old, new, options),
    }
}

struct HunkBuilder<'a> {
    old: &'a [&'a [u8]],
    new: &'a [&'a [u8]],
    show_section: bool,
    done: Vec<Hunk>,
    current: Vec<HunkLine>,
    old_first: usize,
    new_first: usize,
    old_count: usize,
    new_count: usize,
}

impl HunkBuilder<'_> {
    fn open(&mut self, old_at: usize, new_at: usize) {
        self.old_first = old_at;
        self.new_first = new_at;
        self.old_count = 0;
        self.new_count = 0;
    }

    fn context(&mut self, old_at: usize, len: usize) {
        for line in &self.old[old_at..old_at + len] {
            self.current.push(HunkLine { kind: LineKind::Context, text: line.to_vec() });
        }
        self.old_count += len;
        self.new_count += len;
    }

    fn close(&mut self) {
        if self.current.is_empty() {
            return;
        }
        // A count of zero means the start names the line before the position.
        let start = |first: usize, count: usize| if count == 0 { first } else { first + 1 };
        let section = if self.show_section { find_section(&self.old[..self.old_first]) } else { String::new() };
        self.done.push(Hunk {
            old_start: start(self.old_first, self.old_count),
            old_count: self.old_count,
            new_start: start(self.new_first, self.new_count),
            new_count: self.new_count,
            section,
            lines: std::mem::take(&mut self.current),
        });
    }
}

/// Groups an edit script into hunks. Changes separated by no more than twice the context
/// share a hunk, because their context would otherwise overlap or touch.
pub fn build_hunks(old: &[&[u8]], new: &[&[u8]], spans: &[Span], options: &DiffOptions) -> Vec<Hunk> {
    let context = options.context;
    let mut builder = HunkBuilder {
        old,
        new,
        show_section: options.show_section,
        done: Vec::new(),
        current: Vec::new(),
        old_first: 0,
        new_first: 0,
        old_count: 0,
        new_count: 0,
    };
    let mut open = false;
    for (index, span) in spans.iter().enumerate() {
        match span.op {
            Op::Equal => {
                if !open {
                    continue;
                }
                let is_last = index + 1 == spans.len();
                if is_last || span.len > context * 2 {
                    builder.context(span.old_start, span.len.min(context));
                    builder.close();
                    open = false;
                } else {
                    builder.context(span.old_start, span.len);
                }
            }
            Op::Delete | Op::Insert => {
                if !open {
                    let lead = if index == 0 { 0 } else { lead_context(&spans[index - 1], context) };
                    builder.open(span.old_start - lead, span.new_start - lead);
                    builder.context(span.old_start - lead, lead);
                    open = true;
                }
                if span.op == Op::Delete {
                    for line in &builder.old[span.old_start..span.old_start + span.len] {
                        builder.current.push(HunkLine { kind: LineKind::Remove, text: line.to_vec() });
                    }
                    builder.old_count += span.len;
                } else {
                    for line in &builder.new[span.new_start..span.new_start + span.len] {
                        builder.current.push(HunkLine { kind: LineKind::Add, text: line.to_vec() });
                    }
                    builder.new_count += span.len;
                }
            }
        }
    }
    builder.close();
    builder.done
}

fn lead_context(previous: &Span, context: usize) -> usize {
    if previous.op == Op::Equal {
        previous.len.min(context)
    } else {
        0
    }
}

/// The nearest line above that starts with a letter, an underscore or a dollar sign, which
/// is the same rough test GNU diff uses for `-p`.
fn find_section(above: &[&[u8]]) -> String {
    const MAX_LENGTH: usize = 40;
    for line in above.iter().rev() {
        let starts_a_section = line.first().is_some_and(|&byte| byte.is_ascii_alphabetic() || byte == b'_' || byte == b'$');
        if starts_a_section {
            let text = String::from_utf8_lossy(line);
            let trimmed = text.trim_end();
            return trimmed.chars().take(MAX_LENGTH).collect();
        }
    }
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::patch::write_hunk;

    fn text(hunks: &[Hunk]) -> String {
        let mut out = Vec::new();
        for hunk in hunks {
            write_hunk(&mut out, hunk);
        }
        String::from_utf8(out).unwrap()
    }

    fn numbered(count: usize) -> String {
        (1..=count).map(|n| format!("line {n}\n")).collect()
    }

    #[test]
    fn identical_texts_have_no_hunks() {
        assert!(diff_texts(b"a\nb\n", b"a\nb\n", &DiffOptions::default()).is_empty());
    }

    #[test]
    fn one_changed_line_with_context() {
        let old = numbered(9);
        let new = old.replace("line 5\n", "LINE 5\n");
        let hunks = diff_texts(old.as_bytes(), new.as_bytes(), &DiffOptions::default());
        assert_eq!(text(&hunks), "@@ -2,7 +2,7 @@\n line 2\n line 3\n line 4\n-line 5\n+LINE 5\n line 6\n line 7\n line 8\n");
    }

    #[test]
    fn distant_changes_make_two_hunks() {
        let old = numbered(30);
        let new = old.replace("line 3\n", "x\n").replace("line 25\n", "y\n");
        assert_eq!(diff_texts(old.as_bytes(), new.as_bytes(), &DiffOptions::default()).len(), 2);
    }

    #[test]
    fn close_changes_share_a_hunk() {
        let old = numbered(20);
        let new = old.replace("line 5\n", "x\n").replace("line 11\n", "y\n");
        let hunks = diff_texts(old.as_bytes(), new.as_bytes(), &DiffOptions::default());
        assert_eq!(hunks.len(), 1);
        assert_eq!((hunks[0].old_start, hunks[0].old_count), (2, 13));
    }

    #[test]
    fn a_gap_just_over_twice_the_context_splits() {
        let old = numbered(20);
        let new = old.replace("line 5\n", "x\n").replace("line 13\n", "y\n");
        assert_eq!(diff_texts(old.as_bytes(), new.as_bytes(), &DiffOptions::default()).len(), 2);
    }

    #[test]
    fn a_gap_of_exactly_twice_the_context_stays_in_one_hunk() {
        let old = numbered(20);
        let new = old.replace("line 5\n", "x\n").replace("line 12\n", "y\n");
        assert_eq!(diff_texts(old.as_bytes(), new.as_bytes(), &DiffOptions::default()).len(), 1);
    }

    #[test]
    fn zero_context_uses_the_line_before_for_inserts() {
        let hunks = diff_texts(b"a\nb\n", b"a\nnew\nb\n", &DiffOptions { context: 0, ..DiffOptions::default() });
        assert_eq!(text(&hunks), "@@ -1,0 +2 @@\n+new\n");
    }

    #[test]
    fn insert_into_an_empty_file() {
        let hunks = diff_texts(b"", b"a\nb\n", &DiffOptions::default());
        assert_eq!(text(&hunks), "@@ -0,0 +1,2 @@\n+a\n+b\n");
    }

    #[test]
    fn delete_everything() {
        let hunks = diff_texts(b"a\nb\n", b"", &DiffOptions::default());
        assert_eq!(text(&hunks), "@@ -1,2 +0,0 @@\n-a\n-b\n");
    }

    #[test]
    fn a_change_to_the_final_newline_shows_up() {
        let hunks = diff_texts(b"a\nb", b"a\nb\n", &DiffOptions::default());
        assert_eq!(text(&hunks), "@@ -1,2 +1,2 @@\n a\n-b\n\\ No newline at end of file\n+b\n");
    }

    #[test]
    fn trailing_white_space_can_be_ignored() {
        let options = DiffOptions { whitespace: Whitespace::IgnoreTrailing, ..DiffOptions::default() };
        assert!(diff_texts(b"a  \nb\r\n", b"a\nb\n", &options).is_empty());
        assert!(!diff_texts(b"a b\n", b"ab\n", &options).is_empty());
    }

    #[test]
    fn all_white_space_can_be_ignored() {
        let options = DiffOptions { whitespace: Whitespace::IgnoreAll, ..DiffOptions::default() };
        assert!(diff_texts(b"a b\n", b"ab \n", &options).is_empty());
    }

    #[test]
    fn section_names_the_nearest_heading_above() {
        let old = "fn first() {\n  a\n  b\n  c\n  d\n  e\n  f\n}\n";
        let new = old.replace("  e\n", "  E\n");
        let options = DiffOptions { context: 1, show_section: true, ..DiffOptions::default() };
        let hunks = diff_texts(old.as_bytes(), new.as_bytes(), &options);
        assert_eq!(hunks[0].section, "fn first() {");
    }

    #[test]
    fn diff_file_records_the_paths() {
        let file = diff_file(None, b"", Some("new.txt"), b"x\n", &DiffOptions::default());
        assert_eq!(file.old_path, None);
        assert_eq!(file.new_path.as_deref(), Some("new.txt"));
        assert_eq!(file.hunks.len(), 1);
    }
}
