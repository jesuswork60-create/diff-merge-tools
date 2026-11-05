//! The in-memory form of a unified diff, and writing it back out.

/// What a line in a hunk does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    Context,
    Remove,
    Add,
}

/// One line of a hunk. The text keeps its terminator, and a line without one is what the
/// "No newline at end of file" marker describes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HunkLine {
    pub kind: LineKind,
    pub text: Vec<u8>,
}

/// A block of changes. Starts are 1-based, and when a count is zero the start is the line
/// before the position, which is the unified format's own convention.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Hunk {
    pub old_start: usize,
    pub old_count: usize,
    pub new_start: usize,
    pub new_count: usize,
    /// The text after the closing `@@`, often the enclosing function.
    pub section: String,
    pub lines: Vec<HunkLine>,
}

impl Hunk {
    /// The hunk that undoes this one, with removed lines listed first in each change.
    pub fn reversed(&self) -> Hunk {
        let mut lines: Vec<HunkLine> = Vec::with_capacity(self.lines.len());
        let mut pending_adds: Vec<HunkLine> = Vec::new();
        for line in &self.lines {
            match line.kind {
                LineKind::Context => {
                    lines.append(&mut pending_adds);
                    lines.push(line.clone());
                }
                LineKind::Remove => pending_adds.push(HunkLine { kind: LineKind::Add, text: line.text.clone() }),
                LineKind::Add => lines.push(HunkLine { kind: LineKind::Remove, text: line.text.clone() }),
            }
        }
        lines.append(&mut pending_adds);
        Hunk {
            old_start: self.new_start,
            old_count: self.new_count,
            new_start: self.old_start,
            new_count: self.old_count,
            section: self.section.clone(),
            lines,
        }
    }

    /// Lines the hunk expects to find in the file.
    pub fn old_lines(&self) -> impl Iterator<Item = &HunkLine> {
        self.lines.iter().filter(|line| line.kind != LineKind::Add)
    }

    pub fn added(&self) -> usize {
        self.lines.iter().filter(|line| line.kind == LineKind::Add).count()
    }

    pub fn removed(&self) -> usize {
        self.lines.iter().filter(|line| line.kind == LineKind::Remove).count()
    }
}

/// The changes to one file. A missing path stands for `/dev/null`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FilePatch {
    pub old_path: Option<String>,
    pub new_path: Option<String>,
    pub hunks: Vec<Hunk>,
}

impl FilePatch {
    pub fn reversed(&self) -> FilePatch {
        FilePatch {
            old_path: self.new_path.clone(),
            new_path: self.old_path.clone(),
            hunks: self.hunks.iter().map(Hunk::reversed).collect(),
        }
    }

    /// The name to show for this change: the new path, or the old one for a deletion.
    pub fn display_path(&self) -> &str {
        self.new_path.as_deref().or(self.old_path.as_deref()).unwrap_or("/dev/null")
    }
}

/// A parsed patch. `notes` holds things that were seen and skipped, such as binary files.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Patch {
    pub files: Vec<FilePatch>,
    pub notes: Vec<String>,
}

impl Patch {
    pub fn reversed(&self) -> Patch {
        Patch { files: self.files.iter().map(FilePatch::reversed).collect(), notes: self.notes.clone() }
    }
}

fn write_range(out: &mut Vec<u8>, start: usize, count: usize) {
    out.extend_from_slice(start.to_string().as_bytes());
    if count != 1 {
        out.push(b',');
        out.extend_from_slice(count.to_string().as_bytes());
    }
}

/// Writes one hunk in unified format.
pub fn write_hunk(out: &mut Vec<u8>, hunk: &Hunk) {
    out.extend_from_slice(b"@@ -");
    write_range(out, hunk.old_start, hunk.old_count);
    out.extend_from_slice(b" +");
    write_range(out, hunk.new_start, hunk.new_count);
    out.extend_from_slice(b" @@");
    if !hunk.section.is_empty() {
        out.push(b' ');
        out.extend_from_slice(hunk.section.as_bytes());
    }
    out.push(b'\n');
    for line in &hunk.lines {
        out.push(match line.kind {
            LineKind::Context => b' ',
            LineKind::Remove => b'-',
            LineKind::Add => b'+',
        });
        out.extend_from_slice(&line.text);
        if line.text.last() != Some(&b'\n') {
            out.extend_from_slice(b"\n\\ No newline at end of file\n");
        }
    }
}

/// Writes the two header lines and every hunk of a file.
pub fn write_file_patch(out: &mut Vec<u8>, file: &FilePatch) {
    out.extend_from_slice(b"--- ");
    out.extend_from_slice(file.old_path.as_deref().unwrap_or("/dev/null").as_bytes());
    out.extend_from_slice(b"\n+++ ");
    out.extend_from_slice(file.new_path.as_deref().unwrap_or("/dev/null").as_bytes());
    out.push(b'\n');
    for hunk in &file.hunks {
        write_hunk(out, hunk);
    }
}

/// The whole patch as text. Notes are not written, as they aren't part of the format.
pub fn format_patch(patch: &Patch) -> Vec<u8> {
    let mut out = Vec::new();
    for file in &patch.files {
        write_file_patch(&mut out, file);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(kind: LineKind, text: &str) -> HunkLine {
        HunkLine { kind, text: text.as_bytes().to_vec() }
    }

    fn sample() -> Hunk {
        Hunk {
            old_start: 2,
            old_count: 2,
            new_start: 2,
            new_count: 3,
            section: String::new(),
            lines: vec![
                line(LineKind::Context, "keep\n"),
                line(LineKind::Remove, "old\n"),
                line(LineKind::Add, "new\n"),
                line(LineKind::Add, "more\n"),
            ],
        }
    }

    #[test]
    fn writes_ranges_and_lines() {
        let mut out = Vec::new();
        write_hunk(&mut out, &sample());
        assert_eq!(String::from_utf8(out).unwrap(), "@@ -2,2 +2,3 @@\n keep\n-old\n+new\n+more\n");
    }

    #[test]
    fn a_count_of_one_is_left_out() {
        let hunk = Hunk { old_start: 3, old_count: 1, new_start: 3, new_count: 0, ..Hunk::default() };
        let mut out = Vec::new();
        write_hunk(&mut out, &hunk);
        assert_eq!(out, b"@@ -3 +3,0 @@\n");
    }

    #[test]
    fn writes_the_section_after_the_range() {
        let hunk = Hunk { old_start: 1, old_count: 1, new_start: 1, new_count: 1, section: "fn main".into(), lines: vec![] };
        let mut out = Vec::new();
        write_hunk(&mut out, &hunk);
        assert_eq!(out, b"@@ -1 +1 @@ fn main\n");
    }

    #[test]
    fn writes_the_no_newline_marker() {
        let hunk = Hunk { old_start: 1, old_count: 1, new_start: 1, new_count: 1, lines: vec![line(LineKind::Remove, "a"), line(LineKind::Add, "a\n")], ..Hunk::default() };
        let mut out = Vec::new();
        write_hunk(&mut out, &hunk);
        assert_eq!(String::from_utf8(out).unwrap(), "@@ -1 +1 @@\n-a\n\\ No newline at end of file\n+a\n");
    }

    #[test]
    fn reversing_swaps_sides_and_lists_removals_first() {
        let reversed = sample().reversed();
        assert_eq!(reversed.old_count, 3);
        assert_eq!(reversed.new_count, 2);
        let kinds: Vec<LineKind> = reversed.lines.iter().map(|l| l.kind).collect();
        assert_eq!(kinds, vec![LineKind::Context, LineKind::Remove, LineKind::Remove, LineKind::Add]);
        assert_eq!(reversed.lines[3].text, b"old\n");
        assert_eq!(reversed.reversed(), sample());
    }

    #[test]
    fn file_header_uses_dev_null_for_a_missing_side() {
        let file = FilePatch { old_path: None, new_path: Some("x".into()), hunks: vec![] };
        let mut out = Vec::new();
        write_file_patch(&mut out, &file);
        assert_eq!(out, b"--- /dev/null\n+++ x\n");
        assert_eq!(file.display_path(), "x");
        assert_eq!(file.reversed().display_path(), "x");
        assert_eq!(file.reversed().new_path, None);
    }
}
