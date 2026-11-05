//! Showing what changed inside lines, in the `[-old-]{+new+}` notation of `wdiff`.

use crate::myers::{self, Op};
use crate::patch::{write_hunk, Hunk, HunkLine, LineKind};

/// Splits a line into words, runs of space, and single punctuation characters. Bytes above
/// ASCII count as word characters, so multi-byte characters are never cut in two.
pub fn tokenize(line: &[u8]) -> Vec<&[u8]> {
    #[derive(PartialEq, Clone, Copy)]
    enum Class {
        Word,
        Space,
        Other,
    }
    let class = |byte: u8| {
        if byte.is_ascii_alphanumeric() || byte == b'_' || byte >= 0x80 {
            Class::Word
        } else if byte.is_ascii_whitespace() {
            Class::Space
        } else {
            Class::Other
        }
    };
    let mut tokens = Vec::new();
    let mut start = 0;
    for at in 1..=line.len() {
        let boundary = at == line.len() || class(line[at]) != class(line[start]) || class(line[start]) == Class::Other;
        if boundary {
            tokens.push(&line[start..at]);
            start = at;
        }
    }
    tokens
}

fn body(line: &[u8]) -> &[u8] {
    let line = line.strip_suffix(b"\n").unwrap_or(line);
    line.strip_suffix(b"\r").unwrap_or(line)
}

fn wrap_removed(out: &mut Vec<u8>, part: &[u8]) {
    out.extend_from_slice(b"[-");
    out.extend_from_slice(part);
    out.extend_from_slice(b"-]");
}

fn wrap_added(out: &mut Vec<u8>, part: &[u8]) {
    out.extend_from_slice(b"{+");
    out.extend_from_slice(part);
    out.extend_from_slice(b"+}");
}

/// Marks up the differences between two versions of one line. The result has no newline.
pub fn word_diff(old: &[u8], new: &[u8]) -> Vec<u8> {
    let old_tokens = tokenize(body(old));
    let new_tokens = tokenize(body(new));
    let mut out = Vec::new();
    for span in myers::diff(&old_tokens, &new_tokens) {
        match span.op {
            Op::Equal => out.extend(old_tokens[span.old_start..span.old_start + span.len].concat()),
            Op::Delete => wrap_removed(&mut out, &old_tokens[span.old_start..span.old_start + span.len].concat()),
            Op::Insert => wrap_added(&mut out, &new_tokens[span.new_start..span.new_start + span.len].concat()),
        }
    }
    out
}

/// Writes a hunk with changed lines shown word by word. Runs of removed and added lines
/// are paired off when they are the same length; otherwise they are shown whole, because
/// there is no good way to tell which line became which.
pub fn write_word_hunk(out: &mut Vec<u8>, hunk: &Hunk) {
    let header = Hunk { lines: Vec::new(), ..hunk.clone() };
    write_hunk(out, &header);
    let mut at = 0;
    while at < hunk.lines.len() {
        let line = &hunk.lines[at];
        if line.kind == LineKind::Context {
            out.push(b' ');
            out.extend_from_slice(body(&line.text));
            out.push(b'\n');
            at += 1;
            continue;
        }
        let run = |from: usize, kind: LineKind| -> Vec<&HunkLine> { hunk.lines[from..].iter().take_while(|l| l.kind == kind).collect() };
        let removed = run(at, LineKind::Remove);
        let added = run(at + removed.len(), LineKind::Add);
        if removed.len() == added.len() {
            for (old, new) in removed.iter().zip(&added) {
                out.push(b'~');
                out.extend(word_diff(&old.text, &new.text));
                out.push(b'\n');
            }
        } else {
            for old in &removed {
                out.push(b'~');
                wrap_removed(out, body(&old.text));
                out.push(b'\n');
            }
            for new in &added {
                out.push(b'~');
                wrap_added(out, body(&new.text));
                out.push(b'\n');
            }
        }
        at += removed.len() + added.len();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::{diff_texts, DiffOptions};

    fn marked(old: &str, new: &str) -> String {
        String::from_utf8(word_diff(old.as_bytes(), new.as_bytes())).unwrap()
    }

    #[test]
    fn tokens_cover_the_line_and_join_back() {
        let line = "let x_1 = f(a,  b);";
        let tokens = tokenize(line.as_bytes());
        assert_eq!(tokens.concat(), line.as_bytes());
        assert!(tokens.contains(&&b"x_1"[..]));
        assert!(tokens.contains(&&b"  "[..]));
        assert!(tokens.contains(&&b"("[..]));
    }

    #[test]
    fn repeated_punctuation_is_split() {
        assert_eq!(tokenize(b"=>"), vec![&b"="[..], b">"]);
    }

    #[test]
    fn empty_line_has_no_tokens() {
        assert!(tokenize(b"").is_empty());
    }

    #[test]
    fn multi_byte_characters_stay_whole() {
        assert_eq!(marked("caf\u{e9} au lait\n", "caf\u{e8} au lait\n"), "[-caf\u{e9}-]{+caf\u{e8}+} au lait");
    }

    #[test]
    fn marks_replaced_words() {
        assert_eq!(marked("the quick fox\n", "the slow fox\n"), "the [-quick-]{+slow+} fox");
    }

    #[test]
    fn marks_added_and_removed_words() {
        assert_eq!(marked("a b\n", "a b c\n"), "a b{+ c+}");
        assert_eq!(marked("a b c\n", "a c\n"), "a [-b -]c");
    }

    #[test]
    fn line_ending_differences_do_not_show() {
        assert_eq!(marked("same\r\n", "same\n"), "same");
    }

    #[test]
    fn hunks_pair_lines_of_equal_runs() {
        let hunks = diff_texts(b"keep\nold one\n", b"keep\nnew one\n", &DiffOptions::default());
        let mut out = Vec::new();
        write_word_hunk(&mut out, &hunks[0]);
        assert_eq!(String::from_utf8(out).unwrap(), "@@ -1,2 +1,2 @@\n keep\n~[-old-]{+new+} one\n");
    }

    #[test]
    fn hunks_show_unequal_runs_whole() {
        let hunks = diff_texts(b"a b\n", b"x\ny\n", &DiffOptions::default());
        let mut out = Vec::new();
        write_word_hunk(&mut out, &hunks[0]);
        assert_eq!(String::from_utf8(out).unwrap(), "@@ -1 +1,2 @@\n~[-a b-]\n~{+x+}\n~{+y+}\n");
    }
}
