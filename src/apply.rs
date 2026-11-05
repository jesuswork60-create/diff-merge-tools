//! Applying hunks to a text.
//!
//! A hunk rarely needs to be taken at its word. The file may have moved on since the patch
//! was made, so each hunk is looked for near where it says it belongs, and if that fails,
//! again with some of its outer context lines ignored. That is the same idea as `patch`'s
//! offset and fuzz.

use std::borrow::Cow;

use crate::lines::{normalize, split_lines, Whitespace};
use crate::patch::{Hunk, HunkLine, LineKind};

/// How far a hunk may stray from what it says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ApplyOptions {
    /// How many context lines may be ignored at each end of a hunk.
    pub max_fuzz: usize,
    /// How far from its expected position a hunk may land. `None` searches the whole file.
    pub max_offset: Option<usize>,
    /// How lines are compared. Context lines keep the file's own text when this is lenient.
    pub whitespace: Whitespace,
}

impl Default for ApplyOptions {
    fn default() -> Self {
        ApplyOptions { max_fuzz: 2, max_offset: None, whitespace: Whitespace::Exact }
    }
}

/// Why a hunk could not be applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RejectReason {
    /// The lines the hunk expects are nowhere to be found.
    NoMatch,
    /// The lines the hunk would produce are already there.
    AlreadyApplied,
    /// The lines match, but a line without a newline would run into the next one.
    WouldJoinLines,
    /// The file the hunk belongs to is missing or in the way; set by the caller.
    FileProblem,
}

impl RejectReason {
    pub fn describe(self) -> &'static str {
        match self {
            RejectReason::NoMatch => "the expected lines were not found",
            RejectReason::AlreadyApplied => "it looks already applied",
            RejectReason::WouldJoinLines => "a line without a newline would run into the next one",
            RejectReason::FileProblem => "the file could not be used",
        }
    }
}

/// What happened to one hunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HunkStatus {
    /// `line` is where the changed block starts in the result, counting from 1. `offset` is
    /// how far the hunk was from what its header says, and `fuzz` how many context lines
    /// had to be ignored at each end.
    Applied { line: usize, offset: isize, fuzz: usize },
    Rejected(RejectReason),
}

/// The result of applying a list of hunks. Hunks that fail are skipped and the rest still
/// apply, so `text` is valid either way; check [`ApplyReport::is_clean`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplyReport {
    pub text: Vec<u8>,
    pub outcomes: Vec<HunkStatus>,
}

impl ApplyReport {
    pub fn is_clean(&self) -> bool {
        self.outcomes.iter().all(|status| matches!(status, HunkStatus::Applied { .. }))
    }

    pub fn rejected_count(&self) -> usize {
        self.outcomes.iter().filter(|status| matches!(status, HunkStatus::Rejected(_))).count()
    }
}

/// A hunk with its outer context removed for a fuzz level.
struct Trimmed<'h> {
    lines: &'h [HunkLine],
    lead: usize,
}

fn trim(hunk: &Hunk, fuzz: usize) -> Option<Trimmed<'_>> {
    let total = hunk.lines.len();
    let is_context = |line: &&HunkLine| line.kind == LineKind::Context;
    let lead_context = hunk.lines.iter().take_while(is_context).count();
    let trail_context = hunk.lines.iter().rev().take_while(is_context).count();
    let lead = fuzz.min(lead_context);
    let trail = fuzz.min(trail_context);
    if lead + trail > total {
        return None;
    }
    let body = &hunk.lines[lead..total - trail];
    // A body with nothing left to match against would fit anywhere, which is no evidence at all.
    let had_old = hunk.lines.iter().any(|line| line.kind != LineKind::Add);
    if had_old && body.iter().all(|line| line.kind == LineKind::Add) {
        return None;
    }
    Some(Trimmed { lines: body, lead })
}

/// Tries each position near `expected`, nearest first, later positions winning ties.
fn search(expected: isize, low: usize, high: usize, limit: Option<usize>, mut accept: impl FnMut(usize) -> bool) -> Option<usize> {
    if low > high {
        return None;
    }
    let reach = (high as isize - expected).abs().max((expected - low as isize).abs()) as usize;
    let reach = limit.map_or(reach, |limit| limit.min(reach));
    for distance in 0..=reach as isize {
        let later = expected + distance;
        if later >= low as isize && later <= high as isize && accept(later as usize) {
            return Some(later as usize);
        }
        let earlier = expected - distance;
        if distance > 0 && earlier >= low as isize && earlier <= high as isize && accept(earlier as usize) {
            return Some(earlier as usize);
        }
    }
    None
}

fn ends_line(line: &[u8]) -> bool {
    line.last() == Some(&b'\n')
}

/// The lines a hunk leaves behind at a position, taking context from the file.
fn new_side<'a>(body: &'a [HunkLine], file: &[&'a [u8]], at: usize) -> Vec<&'a [u8]> {
    let mut output = Vec::with_capacity(body.len());
    let mut position = at;
    for line in body {
        match line.kind {
            LineKind::Context => {
                output.push(file[position]);
                position += 1;
            }
            LineKind::Remove => position += 1,
            LineKind::Add => output.push(line.text.as_slice()),
        }
    }
    output
}

/// Whether placing these lines here would glue a line without a newline to the next one.
fn would_join(file: &[&[u8]], at: usize, old_len: usize, output: &[&[u8]]) -> bool {
    if output.is_empty() {
        return false;
    }
    if at > 0 && !ends_line(file[at - 1]) {
        return true;
    }
    let (last, inner) = output.split_last().expect("checked not empty");
    if inner.iter().any(|line| !ends_line(line)) {
        return true;
    }
    !ends_line(last) && at + old_len < file.len()
}

/// Applies hunks in order. Positions in a patch refer to the file before any of its hunks
/// ran, so matching is done against the original lines and hunks can't overlap.
pub fn apply_hunks(original: &[u8], hunks: &[Hunk], options: &ApplyOptions) -> ApplyReport {
    let file = split_lines(original);
    let compared: Vec<Cow<'_, [u8]>> = file.iter().map(|line| normalize(line, options.whitespace)).collect();
    let mut text = Vec::with_capacity(original.len());
    let mut outcomes = Vec::with_capacity(hunks.len());
    let mut cursor = 0;
    let mut written_lines = 0;
    let mut last_offset = 0isize;

    for hunk in hunks {
        let header_index = if hunk.old_count == 0 { hunk.old_start } else { hunk.old_start.saturating_sub(1) } as isize;
        let mut found = None;
        let mut saw_join = false;
        for fuzz in 0..=options.max_fuzz {
            let Some(trimmed) = trim(hunk, fuzz) else { continue };
            if fuzz > 0 && trim(hunk, fuzz - 1).is_some_and(|previous| previous.lines.len() == trimmed.lines.len()) {
                continue;
            }
            let wanted: Vec<Cow<'_, [u8]>> = trimmed
                .lines
                .iter()
                .filter(|line| line.kind != LineKind::Add)
                .map(|line| normalize(&line.text, options.whitespace))
                .collect();
            if wanted.len() > file.len() {
                continue;
            }
            let expected = header_index + trimmed.lead as isize;
            // With no old lines to anchor on, every position "matches", so only the stated one is tried.
            let limit = if wanted.is_empty() { Some(0) } else { options.max_offset };
            let position = search(expected + last_offset, cursor, file.len() - wanted.len(), limit, |at| {
                if compared[at..at + wanted.len()] != wanted[..] {
                    return false;
                }
                if would_join(&file, at, wanted.len(), &new_side(trimmed.lines, &file, at)) {
                    saw_join = true;
                    return false;
                }
                true
            });
            if let Some(at) = position {
                found = Some((at, wanted.len(), expected, fuzz, trimmed));
                break;
            }
        }
        match found {
            Some((at, old_len, expected, fuzz, trimmed)) => {
                for line in &file[cursor..at] {
                    text.extend_from_slice(line);
                }
                written_lines += at - cursor;
                let output = new_side(trimmed.lines, &file, at);
                for line in &output {
                    text.extend_from_slice(line);
                }
                let offset = at as isize - expected;
                outcomes.push(HunkStatus::Applied { line: written_lines + 1, offset, fuzz });
                written_lines += output.len();
                cursor = at + old_len;
                last_offset = offset;
            }
            None => {
                let reason = if saw_join {
                    RejectReason::WouldJoinLines
                } else if looks_applied(hunk, &compared, options.whitespace) {
                    RejectReason::AlreadyApplied
                } else {
                    RejectReason::NoMatch
                };
                outcomes.push(HunkStatus::Rejected(reason));
            }
        }
    }
    for line in &file[cursor..] {
        text.extend_from_slice(line);
    }
    ApplyReport { text, outcomes }
}

/// Whether the file already holds what the hunk would produce.
fn looks_applied(hunk: &Hunk, compared: &[Cow<'_, [u8]>], mode: Whitespace) -> bool {
    let result: Vec<Cow<'_, [u8]>> = hunk
        .lines
        .iter()
        .filter(|line| line.kind != LineKind::Remove)
        .map(|line| normalize(&line.text, mode))
        .collect();
    !result.is_empty() && hunk.removed() + hunk.added() > 0 && compared.windows(result.len()).any(|window| window == &result[..])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::{diff_texts, DiffOptions};

    fn line(kind: LineKind, text: &str) -> HunkLine {
        HunkLine { kind, text: text.as_bytes().to_vec() }
    }

    fn hunk(old_start: usize, old_count: usize, new_start: usize, new_count: usize, lines: Vec<HunkLine>) -> Hunk {
        Hunk { old_start, old_count, new_start, new_count, section: String::new(), lines }
    }

    fn change_b() -> Hunk {
        hunk(2, 3, 2, 3, vec![line(LineKind::Context, "a\n"), line(LineKind::Remove, "b\n"), line(LineKind::Add, "B\n"), line(LineKind::Context, "c\n")])
    }

    fn apply(text: &str, hunks: &[Hunk]) -> ApplyReport {
        apply_hunks(text.as_bytes(), hunks, &ApplyOptions::default())
    }

    #[test]
    fn exact_position() {
        let report = apply("x\na\nb\nc\ny\n", &[change_b()]);
        assert_eq!(report.text, b"x\na\nB\nc\ny\n");
        assert_eq!(report.outcomes, vec![HunkStatus::Applied { line: 2, offset: 0, fuzz: 0 }]);
    }

    #[test]
    fn finds_a_moved_hunk_and_reports_the_offset() {
        let report = apply("p\nq\nr\nx\na\nb\nc\ny\n", &[change_b()]);
        assert_eq!(report.text, b"p\nq\nr\nx\na\nB\nc\ny\n");
        assert_eq!(report.outcomes, vec![HunkStatus::Applied { line: 5, offset: 3, fuzz: 0 }]);
    }

    #[test]
    fn finds_a_hunk_that_moved_up() {
        let report = apply("a\nb\nc\n", &[change_b()]);
        assert_eq!(report.outcomes, vec![HunkStatus::Applied { line: 1, offset: -1, fuzz: 0 }]);
    }

    #[test]
    fn the_nearest_match_wins() {
        let report = apply("a\nb\nc\nz\na\nb\nc\n", &[hunk(5, 3, 5, 3, change_b().lines)]);
        assert_eq!(report.text, b"a\nb\nc\nz\na\nB\nc\n");
    }

    #[test]
    fn a_later_hunk_starts_from_the_offset_of_the_one_before() {
        let first = hunk(1, 1, 1, 1, vec![line(LineKind::Remove, "one\n"), line(LineKind::Add, "ONE\n")]);
        let second = hunk(2, 1, 2, 1, vec![line(LineKind::Remove, "same\n"), line(LineKind::Add, "SAME\n")]);
        let report = apply("same\nextra\none\nmiddle\nsame\nsame\n", &[first, second]);
        assert_eq!(report.text, b"same\nextra\nONE\nmiddle\nSAME\nsame\n");
    }

    #[test]
    fn offset_limit_is_honoured() {
        let options = ApplyOptions { max_offset: Some(1), ..ApplyOptions::default() };
        let report = apply_hunks(b"p\nq\nr\nx\na\nb\nc\n", &[change_b()], &options);
        assert_eq!(report.outcomes, vec![HunkStatus::Rejected(RejectReason::NoMatch)]);
    }

    #[test]
    fn fuzz_ignores_outer_context() {
        let report = apply("x\nA-changed\nb\nc\n", &[change_b()]);
        assert_eq!(report.text, b"x\nA-changed\nB\nc\n");
        assert_eq!(report.outcomes, vec![HunkStatus::Applied { line: 3, offset: 0, fuzz: 1 }]);
    }

    #[test]
    fn no_fuzz_means_no_fuzz() {
        let options = ApplyOptions { max_fuzz: 0, ..ApplyOptions::default() };
        let report = apply_hunks(b"x\nA-changed\nb\nc\n", &[change_b()], &options);
        assert_eq!(report.rejected_count(), 1);
        assert_eq!(report.text, b"x\nA-changed\nb\nc\n");
    }

    #[test]
    fn fuzz_never_removes_changed_lines() {
        let report = apply("x\na\nbb\nc\ny\n", &[change_b()]);
        assert_eq!(report.outcomes, vec![HunkStatus::Rejected(RejectReason::NoMatch)]);
    }

    #[test]
    fn a_failed_hunk_leaves_the_others_working() {
        let good = hunk(1, 1, 1, 1, vec![line(LineKind::Remove, "a\n"), line(LineKind::Add, "A\n")]);
        let bad = hunk(2, 1, 2, 1, vec![line(LineKind::Remove, "missing\n"), line(LineKind::Add, "M\n")]);
        let last = hunk(3, 1, 3, 1, vec![line(LineKind::Remove, "c\n"), line(LineKind::Add, "C\n")]);
        let report = apply("a\nb\nc\n", &[good, bad, last]);
        assert_eq!(report.text, b"A\nb\nC\n");
        assert_eq!(report.rejected_count(), 1);
        assert!(!report.is_clean());
    }

    #[test]
    fn recognises_a_hunk_already_applied() {
        let report = apply("x\na\nB\nc\ny\n", &[change_b()]);
        assert_eq!(report.outcomes, vec![HunkStatus::Rejected(RejectReason::AlreadyApplied)]);
    }

    #[test]
    fn pure_insert_uses_its_position() {
        let insert = hunk(1, 0, 2, 1, vec![line(LineKind::Add, "new\n")]);
        assert_eq!(apply("a\nb\n", &[insert]).text, b"a\nnew\nb\n");
    }

    #[test]
    fn insert_into_empty_text() {
        let insert = hunk(0, 0, 1, 1, vec![line(LineKind::Add, "new\n")]);
        assert_eq!(apply("", &[insert]).text, b"new\n");
    }

    #[test]
    fn refuses_to_add_after_a_line_without_a_newline() {
        let insert = hunk(1, 0, 2, 1, vec![line(LineKind::Add, "new\n")]);
        let report = apply("last", &[insert]);
        assert_eq!(report.outcomes, vec![HunkStatus::Rejected(RejectReason::WouldJoinLines)]);
        assert_eq!(report.text, b"last");
    }

    #[test]
    fn refuses_a_new_last_line_with_lines_after_it() {
        let change = hunk(1, 1, 1, 1, vec![line(LineKind::Remove, "a\n"), line(LineKind::Add, "A")]);
        let report = apply("a\nb\n", &[change]);
        assert_eq!(report.outcomes, vec![HunkStatus::Rejected(RejectReason::WouldJoinLines)]);
    }

    #[test]
    fn the_final_newline_can_be_added_and_removed() {
        let adds = hunk(1, 1, 1, 1, vec![line(LineKind::Remove, "a"), line(LineKind::Add, "a\n")]);
        assert_eq!(apply("a", &[adds]).text, b"a\n");
        let removes = hunk(1, 1, 1, 1, vec![line(LineKind::Remove, "a\n"), line(LineKind::Add, "a")]);
        assert_eq!(apply("a\n", &[removes]).text, b"a");
    }

    #[test]
    fn a_line_without_a_newline_does_not_match_one_with() {
        let change = hunk(1, 1, 1, 1, vec![line(LineKind::Remove, "a\n"), line(LineKind::Add, "b\n")]);
        assert_eq!(apply("a", &[change]).rejected_count(), 1);
    }

    #[test]
    fn lenient_white_space_keeps_the_files_context() {
        let options = ApplyOptions { whitespace: Whitespace::IgnoreTrailing, ..ApplyOptions::default() };
        let report = apply_hunks(b"x\na  \r\nb\r\nc\n", &[change_b()], &options);
        assert_eq!(report.text, b"x\na  \r\nB\nc\n");
        assert!(report.is_clean());
    }

    #[test]
    fn hunks_can_not_overlap() {
        let first = hunk(1, 2, 1, 2, vec![line(LineKind::Remove, "a\n"), line(LineKind::Remove, "b\n"), line(LineKind::Add, "A\n"), line(LineKind::Add, "B\n")]);
        let second = hunk(2, 1, 2, 1, vec![line(LineKind::Remove, "b\n"), line(LineKind::Add, "BB\n")]);
        let report = apply("a\nb\n", &[first, second]);
        assert_eq!(report.text, b"A\nB\n");
        assert_eq!(report.rejected_count(), 1);
    }

    #[test]
    fn hunks_from_a_real_diff_round_trip() {
        let old = "1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n11\n12\n13\n14\n15\n16\n";
        let new = "1\ntwo\n3\n4\n5\n6\n7\n8\n9\n10\n11\n12\n13\n14\nfifteen\nextra\n16\n";
        let hunks = diff_texts(old.as_bytes(), new.as_bytes(), &DiffOptions::default());
        assert_eq!(hunks.len(), 2);
        assert_eq!(apply(old, &hunks).text, new.as_bytes());
        let back: Vec<Hunk> = hunks.iter().map(Hunk::reversed).collect();
        assert_eq!(apply(new, &back).text, old.as_bytes());
    }
}
