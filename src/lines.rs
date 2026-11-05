//! Splitting text into lines and comparing them with some leniency.

use std::borrow::Cow;

/// Splits after each newline. Each piece keeps its terminator, so joining them gives the
/// input back, and a last line without one stays distinct from the same line with one.
pub fn split_lines(text: &[u8]) -> Vec<&[u8]> {
    text.split_inclusive(|&byte| byte == b'\n').collect()
}

/// A file counts as binary when its first 8000 bytes hold a NUL.
pub fn is_binary(text: &[u8]) -> bool {
    text.iter().take(8000).any(|&byte| byte == 0)
}

/// How much white space matters when deciding if two lines are the same.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Whitespace {
    #[default]
    Exact,
    /// White space at the end of a line is ignored, which covers CRLF against LF as well.
    IgnoreTrailing,
    /// All white space is ignored.
    IgnoreAll,
}

fn is_space(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\r' | b'\n' | 0x0b | 0x0c)
}

/// The form of a line that is compared under this mode.
pub fn normalize(line: &[u8], mode: Whitespace) -> Cow<'_, [u8]> {
    match mode {
        Whitespace::Exact => Cow::Borrowed(line),
        Whitespace::IgnoreTrailing => {
            let end = line.iter().rposition(|&byte| !is_space(byte)).map_or(0, |at| at + 1);
            Cow::Borrowed(&line[..end])
        }
        Whitespace::IgnoreAll => Cow::Owned(line.iter().copied().filter(|&byte| !is_space(byte)).collect()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_keeps_terminators_and_joins_back() {
        let text = b"one\ntwo\r\n\nlast";
        let lines = split_lines(text);
        assert_eq!(lines, vec![&b"one\n"[..], b"two\r\n", b"\n", b"last"]);
        assert_eq!(lines.concat(), text);
    }

    #[test]
    fn split_of_empty_text_has_no_lines() {
        assert!(split_lines(b"").is_empty());
    }

    #[test]
    fn nul_means_binary() {
        assert!(is_binary(b"abc\0def"));
        assert!(!is_binary("caf\u{e9}\n".as_bytes()));
    }

    #[test]
    fn a_nul_far_into_the_file_is_not_seen() {
        let mut text = vec![b'a'; 9000];
        text.push(0);
        assert!(!is_binary(&text));
    }

    #[test]
    fn normalize_trailing() {
        assert_eq!(&*normalize(b"a b \t\r\n", Whitespace::IgnoreTrailing), b"a b");
        assert_eq!(&*normalize(b"  \n", Whitespace::IgnoreTrailing), b"");
    }

    #[test]
    fn normalize_all() {
        assert_eq!(&*normalize(b" a\tb c\n", Whitespace::IgnoreAll), b"abc");
    }

    #[test]
    fn exact_keeps_everything() {
        assert_eq!(&*normalize(b" a\n", Whitespace::Exact), b" a\n");
    }
}
