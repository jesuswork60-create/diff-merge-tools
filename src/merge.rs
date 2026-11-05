//! Three-way merging of texts, following the diff3 approach of Khanna, Myers and Pierce.
//!
//! Lines of the base that survive unchanged on both sides are fixed points. Whatever lies
//! between two fixed points was changed by one side, by both in the same way, or by both
//! in different ways, and only the last of those is a conflict.

use crate::diff::line_spans;
use crate::lines::{split_lines, Whitespace};
use crate::myers::Op;

/// How conflicts are written out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ConflictStyle {
    /// Both sides, with markers.
    #[default]
    Merge,
    /// Both sides and the base text between them.
    Diff3,
}

/// Decides a conflict with no markers at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Favor {
    #[default]
    None,
    Ours,
    Theirs,
    /// Keeps the lines from both sides, ours first.
    Union,
}

/// The names written after the conflict markers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Labels {
    pub ours: String,
    pub base: String,
    pub theirs: String,
}

impl Default for Labels {
    fn default() -> Self {
        Labels { ours: "ours".into(), base: "base".into(), theirs: "theirs".into() }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MergeOptions {
    pub style: ConflictStyle,
    pub favor: Favor,
    pub labels: Labels,
}

/// A conflict left in the output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConflictSpot {
    /// The line of the opening marker in the merged text, counting from 1.
    pub line: usize,
    pub ours_lines: usize,
    pub base_lines: usize,
    pub theirs_lines: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MergeResult {
    pub text: Vec<u8>,
    pub conflicts: Vec<ConflictSpot>,
    /// Conflicts settled by the `favor` option.
    pub auto_resolved: usize,
    /// Regions both sides changed in the same way.
    pub same_changes: usize,
}

impl MergeResult {
    pub fn is_clean(&self) -> bool {
        self.conflicts.is_empty()
    }
}

/// One stretch of a merge, before it is written out.
#[derive(Debug, PartialEq, Eq)]
enum Region<'a> {
    Stable(&'a [u8]),
    Take(&'a [&'a [u8]]),
    Same(&'a [&'a [u8]]),
    Conflict { base: &'a [&'a [u8]], ours: &'a [&'a [u8]], theirs: &'a [&'a [u8]] },
}

/// For each base line, where the same line sits on the other side, if it survived.
fn survivors(base: &[&[u8]], other: &[&[u8]]) -> Vec<Option<usize>> {
    let mut found = vec![None; base.len()];
    for span in line_spans(base, other, Whitespace::Exact) {
        if span.op == Op::Equal {
            for step in 0..span.len {
                found[span.old_start + step] = Some(span.new_start + step);
            }
        }
    }
    found
}

fn regions<'a>(base: &'a [&'a [u8]], ours: &'a [&'a [u8]], theirs: &'a [&'a [u8]]) -> Vec<Region<'a>> {
    let in_ours = survivors(base, ours);
    let in_theirs = survivors(base, theirs);
    let mut out = Vec::new();
    let (mut b, mut o, mut t) = (0, 0, 0);
    while b < base.len() || o < ours.len() || t < theirs.len() {
        let next_fixed = (b..base.len()).find_map(|at| Some((at, in_ours[at]?, in_theirs[at]?)));
        let (fb, fo, ft) = next_fixed.unwrap_or((base.len(), ours.len(), theirs.len()));
        if fb == b && fo == o && ft == t {
            out.push(Region::Stable(base[b]));
            b += 1;
            o += 1;
            t += 1;
            continue;
        }
        let (base_part, ours_part, theirs_part) = (&base[b..fb], &ours[o..fo], &theirs[t..ft]);
        if ours_part == theirs_part {
            out.push(Region::Same(ours_part));
        } else if ours_part == base_part {
            out.push(Region::Take(theirs_part));
        } else if theirs_part == base_part {
            out.push(Region::Take(ours_part));
        } else {
            out.push(Region::Conflict { base: base_part, ours: ours_part, theirs: theirs_part });
        }
        (b, o, t) = (fb, fo, ft);
    }
    out
}

struct Writer {
    text: Vec<u8>,
    lines: usize,
}

impl Writer {
    fn lines(&mut self, lines: &[&[u8]]) {
        for line in lines {
            self.line(line);
        }
    }

    fn line(&mut self, line: &[u8]) {
        self.text.extend_from_slice(line);
        self.lines += 1;
    }

    /// Makes sure what follows starts a new line. A side of a conflict can end in a line
    /// with no newline, and a marker glued to it would no longer be a marker.
    fn close_line(&mut self) {
        if !self.text.is_empty() && self.text.last() != Some(&b'\n') {
            self.text.push(b'\n');
        }
    }

    fn marker(&mut self, sign: u8, label: &str) {
        self.close_line();
        self.text.extend(std::iter::repeat_n(sign, 7));
        if !label.is_empty() {
            self.text.push(b' ');
            self.text.extend_from_slice(label.as_bytes());
        }
        self.text.push(b'\n');
        self.lines += 1;
    }
}

/// Merges two descendants of a common base.
pub fn merge(base: &[u8], ours: &[u8], theirs: &[u8], options: &MergeOptions) -> MergeResult {
    let base_lines = split_lines(base);
    let our_lines = split_lines(ours);
    let their_lines = split_lines(theirs);
    let mut writer = Writer { text: Vec::with_capacity(base.len()), lines: 0 };
    let mut result = MergeResult::default();
    for region in regions(&base_lines, &our_lines, &their_lines) {
        match region {
            Region::Stable(line) => writer.line(line),
            Region::Take(lines) => writer.lines(lines),
            Region::Same(lines) => {
                result.same_changes += 1;
                writer.lines(lines);
            }
            Region::Conflict { base, ours, theirs } => match options.favor {
                Favor::Ours => {
                    result.auto_resolved += 1;
                    writer.lines(ours);
                }
                Favor::Theirs => {
                    result.auto_resolved += 1;
                    writer.lines(theirs);
                }
                Favor::Union => {
                    result.auto_resolved += 1;
                    writer.lines(ours);
                    if !theirs.is_empty() {
                        writer.close_line();
                    }
                    writer.lines(theirs);
                }
                Favor::None => {
                    result.conflicts.push(ConflictSpot {
                        line: writer.lines + 1,
                        ours_lines: ours.len(),
                        base_lines: base.len(),
                        theirs_lines: theirs.len(),
                    });
                    writer.marker(b'<', &options.labels.ours);
                    writer.lines(ours);
                    if options.style == ConflictStyle::Diff3 {
                        writer.marker(b'|', &options.labels.base);
                        writer.lines(base);
                    }
                    writer.marker(b'=', "");
                    writer.lines(theirs);
                    writer.marker(b'>', &options.labels.theirs);
                }
            },
        }
    }
    result.text = writer.text;
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(result: &MergeResult) -> String {
        String::from_utf8(result.text.clone()).unwrap()
    }

    fn plain(base: &str, ours: &str, theirs: &str) -> MergeResult {
        merge(base.as_bytes(), ours.as_bytes(), theirs.as_bytes(), &MergeOptions::default())
    }

    const BASE: &str = "a\nb\nc\nd\ne\nf\ng\n";

    #[test]
    fn nothing_changed() {
        let result = plain(BASE, BASE, BASE);
        assert!(result.is_clean());
        assert_eq!(text(&result), BASE);
    }

    #[test]
    fn takes_a_change_from_either_side() {
        let changed = BASE.replace("c\n", "C\n");
        assert_eq!(text(&plain(BASE, &changed, BASE)), changed);
        assert_eq!(text(&plain(BASE, BASE, &changed)), changed);
    }

    #[test]
    fn combines_changes_in_different_places() {
        let ours = BASE.replace("b\n", "B\n");
        let theirs = BASE.replace("f\n", "F\n");
        let result = plain(BASE, &ours, &theirs);
        assert!(result.is_clean());
        assert_eq!(text(&result), "a\nB\nc\nd\ne\nF\ng\n");
    }

    #[test]
    fn the_same_change_on_both_sides_is_no_conflict() {
        let changed = BASE.replace("d\n", "D\n");
        let result = plain(BASE, &changed, &changed);
        assert!(result.is_clean());
        assert_eq!(result.same_changes, 1);
        assert_eq!(text(&result), changed);
    }

    #[test]
    fn different_changes_conflict() {
        let result = plain("a\nb\nc\n", "a\nX\nc\n", "a\nY\nc\n");
        assert_eq!(text(&result), "a\n<<<<<<< ours\nX\n=======\nY\n>>>>>>> theirs\nc\n");
        assert_eq!(result.conflicts, vec![ConflictSpot { line: 2, ours_lines: 1, base_lines: 1, theirs_lines: 1 }]);
    }

    #[test]
    fn diff3_style_shows_the_base() {
        let options = MergeOptions { style: ConflictStyle::Diff3, ..MergeOptions::default() };
        let result = merge(b"a\nb\nc\n", b"a\nX\nc\n", b"a\nY\nc\n", &options);
        assert_eq!(text(&result), "a\n<<<<<<< ours\nX\n||||||| base\nb\n=======\nY\n>>>>>>> theirs\nc\n");
    }

    #[test]
    fn favor_settles_conflicts() {
        let run = |favor| {
            let options = MergeOptions { favor, ..MergeOptions::default() };
            merge(b"a\nb\nc\n", b"a\nX\nc\n", b"a\nY\nc\n", &options)
        };
        assert_eq!(text(&run(Favor::Ours)), "a\nX\nc\n");
        assert_eq!(text(&run(Favor::Theirs)), "a\nY\nc\n");
        assert_eq!(text(&run(Favor::Union)), "a\nX\nY\nc\n");
        assert_eq!(run(Favor::Union).auto_resolved, 1);
        assert!(run(Favor::Union).is_clean());
    }

    #[test]
    fn labels_are_used() {
        let options = MergeOptions {
            labels: Labels { ours: "HEAD".into(), base: "old".into(), theirs: "topic".into() },
            style: ConflictStyle::Diff3,
            ..MergeOptions::default()
        };
        let merged = text(&merge(b"b\n", b"X\n", b"Y\n", &options));
        assert_eq!(merged, "<<<<<<< HEAD\nX\n||||||| old\nb\n=======\nY\n>>>>>>> topic\n");
    }

    #[test]
    fn both_appending_different_lines_conflicts() {
        let result = plain("a\n", "a\nx\n", "a\ny\n");
        assert_eq!(text(&result), "a\n<<<<<<< ours\nx\n=======\ny\n>>>>>>> theirs\n");
        assert_eq!(result.conflicts[0].base_lines, 0);
    }

    #[test]
    fn insert_on_one_side_and_edit_nearby_on_the_other() {
        let result = plain("a\nb\nc\nd\n", "a\nnew\nb\nc\nd\n", "a\nb\nc\nD\n");
        assert!(result.is_clean());
        assert_eq!(text(&result), "a\nnew\nb\nc\nD\n");
    }

    #[test]
    fn edits_to_adjacent_lines_conflict() {
        let result = plain("a\nb\nc\n", "A\nb\nc\n", "a\nB\nc\n");
        assert_eq!(result.conflicts.len(), 1);
        assert_eq!(result.conflicts[0].base_lines, 2);
    }

    #[test]
    fn delete_against_edit_conflicts() {
        let result = plain("a\nb\nc\n", "a\nc\n", "a\nB\nc\n");
        assert_eq!(text(&result), "a\n<<<<<<< ours\n=======\nB\n>>>>>>> theirs\nc\n");
        assert_eq!(result.conflicts[0].ours_lines, 0);
    }

    #[test]
    fn delete_on_both_sides_is_clean() {
        let result = plain("a\nb\nc\n", "a\nc\n", "a\nc\n");
        assert!(result.is_clean());
        assert_eq!(text(&result), "a\nc\n");
    }

    #[test]
    fn markers_start_on_their_own_line_when_a_side_has_no_final_newline() {
        let result = plain("a\nb", "a\nX", "a\nY");
        assert_eq!(text(&result), "a\n<<<<<<< ours\nX\n=======\nY\n>>>>>>> theirs\n");
    }

    #[test]
    fn union_does_not_glue_a_side_without_a_final_newline() {
        let options = MergeOptions { favor: Favor::Union, ..MergeOptions::default() };
        assert_eq!(merge(b"b", b"X", b"Y", &options).text, b"X\nY");
    }

    #[test]
    fn one_side_only_changes_the_final_newline() {
        let result = plain("a\n", "a", "a\n");
        assert!(result.is_clean());
        assert_eq!(result.text, b"a");
    }

    #[test]
    fn empty_inputs() {
        assert_eq!(text(&plain("", "", "")), "");
        assert_eq!(text(&plain("", "x\n", "")), "x\n");
        assert_eq!(text(&plain("", "", "x\n")), "x\n");
        assert_eq!(plain("", "x\n", "y\n").conflicts.len(), 1);
    }

    #[test]
    fn second_conflict_line_number_counts_the_markers_before_it() {
        let base = "a\nm\nn\no\nb\n";
        let result = plain(base, "X\nm\nn\no\nP\n", "Y\nm\nn\no\nQ\n");
        let lines: Vec<usize> = result.conflicts.iter().map(|spot| spot.line).collect();
        assert_eq!(lines, vec![1, 9]);
        let merged = text(&result);
        assert_eq!(merged.lines().nth(8), Some("<<<<<<< ours"));
    }
}
