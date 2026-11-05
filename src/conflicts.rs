//! Finding conflict markers in a file and settling them afterwards.

use std::fmt;

use crate::lines::split_lines;

/// Text between markers, as complete lines.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ConflictBlock {
    /// The line of the opening marker, counting from 1.
    pub line: usize,
    pub ours: Vec<Vec<u8>>,
    pub base: Option<Vec<Vec<u8>>>,
    pub theirs: Vec<Vec<u8>>,
}

/// Part of a file with conflicts in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Piece {
    Text(Vec<u8>),
    Conflict(ConflictBlock),
}

/// Markers that don't form a proper block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarkerError {
    pub line: usize,
    pub message: String,
}

impl fmt::Display for MarkerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "line {}: {}", self.line, self.message)
    }
}

impl std::error::Error for MarkerError {}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Marker {
    Open,
    Base,
    Middle,
    Close,
}

/// Which marker a line is. Only seven signs count, followed by a space or the line's end,
/// so that something like a Markdown underline of `=` characters is left alone unless it
/// is exactly seven long.
fn classify(line: &[u8]) -> Option<Marker> {
    let body = line.strip_suffix(b"\n").unwrap_or(line);
    let body = body.strip_suffix(b"\r").unwrap_or(body);
    if body.len() < 7 {
        return None;
    }
    let sign = body[0];
    let marker = match sign {
        b'<' => Marker::Open,
        b'|' => Marker::Base,
        b'=' => Marker::Middle,
        b'>' => Marker::Close,
        _ => return None,
    };
    if body[..7].iter().any(|&byte| byte != sign) {
        return None;
    }
    match (marker, body.get(7)) {
        (_, None) => Some(marker),
        (Marker::Middle, Some(_)) => None,
        (_, Some(b' ')) => Some(marker),
        _ => None,
    }
}

#[derive(Clone, Copy)]
enum Section {
    Outside,
    Ours,
    Base,
    Theirs,
}

/// Splits a file into plain text and conflicts.
///
/// A lone `=======` outside any block is plain text, because that is common in ordinary
/// files, whereas an unbalanced opening or closing marker is an error.
pub fn parse_conflicts(text: &[u8]) -> Result<Vec<Piece>, MarkerError> {
    let mut pieces = Vec::new();
    let mut plain = Vec::new();
    let mut block = ConflictBlock::default();
    let mut section = Section::Outside;
    let fail = |line: usize, message: &str| Err(MarkerError { line, message: message.to_string() });
    for (index, line) in split_lines(text).into_iter().enumerate() {
        let number = index + 1;
        let marker = classify(line);
        match (section, marker) {
            (Section::Outside, Some(Marker::Open)) => {
                if !plain.is_empty() {
                    pieces.push(Piece::Text(std::mem::take(&mut plain)));
                }
                block = ConflictBlock { line: number, ..ConflictBlock::default() };
                section = Section::Ours;
            }
            (Section::Outside, Some(Marker::Close)) => return fail(number, "closing marker with no opening marker"),
            (Section::Outside, _) => plain.extend_from_slice(line),
            (Section::Ours, Some(Marker::Base)) => {
                block.base = Some(Vec::new());
                section = Section::Base;
            }
            (Section::Ours | Section::Base, Some(Marker::Middle)) => section = Section::Theirs,
            (Section::Theirs, Some(Marker::Close)) => {
                pieces.push(Piece::Conflict(std::mem::take(&mut block)));
                section = Section::Outside;
            }
            (_, Some(_)) => return fail(number, "marker out of order"),
            (Section::Ours, None) => block.ours.push(line.to_vec()),
            (Section::Base, None) => block.base.get_or_insert_with(Vec::new).push(line.to_vec()),
            (Section::Theirs, None) => block.theirs.push(line.to_vec()),
        }
    }
    if !matches!(section, Section::Outside) {
        return fail(block.line, "conflict is not closed");
    }
    if !plain.is_empty() {
        pieces.push(Piece::Text(plain));
    }
    Ok(pieces)
}

/// Which text to keep when settling conflicts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolution {
    Ours,
    Theirs,
    /// The base text, which needs the file to have been written in diff3 style.
    Base,
    /// Ours followed by theirs.
    Union,
}

/// Replaces every conflict with the chosen side and returns the text.
pub fn resolve(text: &[u8], choice: Resolution) -> Result<Vec<u8>, MarkerError> {
    let mut out = Vec::with_capacity(text.len());
    for piece in parse_conflicts(text)? {
        match piece {
            Piece::Text(plain) => out.extend_from_slice(&plain),
            Piece::Conflict(block) => {
                let chosen: Vec<Vec<u8>> = match choice {
                    Resolution::Ours => block.ours,
                    Resolution::Theirs => block.theirs,
                    Resolution::Union => block.ours.into_iter().chain(block.theirs).collect(),
                    Resolution::Base => match block.base {
                        Some(base) => base,
                        None => {
                            return Err(MarkerError { line: block.line, message: "this conflict has no base section".to_string() });
                        }
                    },
                };
                out.extend(chosen.into_iter().flatten());
            }
        }
    }
    Ok(out)
}

/// The conflicts in a text.
pub fn list_conflicts(text: &[u8]) -> Result<Vec<ConflictBlock>, MarkerError> {
    Ok(parse_conflicts(text)?
        .into_iter()
        .filter_map(|piece| match piece {
            Piece::Conflict(block) => Some(block),
            Piece::Text(_) => None,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &[u8] = b"top\n<<<<<<< ours\nX\n||||||| base\nB\n=======\nY\n>>>>>>> theirs\nend\n";

    #[test]
    fn finds_blocks_and_sections() {
        let blocks = list_conflicts(SAMPLE).unwrap();
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].line, 2);
        assert_eq!(blocks[0].ours, vec![b"X\n".to_vec()]);
        assert_eq!(blocks[0].base, Some(vec![b"B\n".to_vec()]));
        assert_eq!(blocks[0].theirs, vec![b"Y\n".to_vec()]);
    }

    #[test]
    fn resolves_each_way() {
        assert_eq!(resolve(SAMPLE, Resolution::Ours).unwrap(), b"top\nX\nend\n");
        assert_eq!(resolve(SAMPLE, Resolution::Theirs).unwrap(), b"top\nY\nend\n");
        assert_eq!(resolve(SAMPLE, Resolution::Base).unwrap(), b"top\nB\nend\n");
        assert_eq!(resolve(SAMPLE, Resolution::Union).unwrap(), b"top\nX\nY\nend\n");
    }

    #[test]
    fn base_needs_a_base_section() {
        let text = b"<<<<<<< a\nX\n=======\nY\n>>>>>>> b\n";
        assert!(resolve(text, Resolution::Base).is_err());
    }

    #[test]
    fn plain_text_is_unchanged() {
        assert_eq!(resolve(b"a\nb", Resolution::Ours).unwrap(), b"a\nb");
        assert!(list_conflicts(b"a\n").unwrap().is_empty());
    }

    #[test]
    fn a_lone_separator_is_just_text() {
        assert_eq!(list_conflicts(b"title\n=======\nmore\n").unwrap().len(), 0);
        assert_eq!(resolve(b"title\n=======\nmore\n", Resolution::Ours).unwrap(), b"title\n=======\nmore\n");
    }

    #[test]
    fn near_markers_are_not_markers() {
        let text = b"<<<<<<<< eight\n<<<<<<x\n>>>>>>>nospace\n======== long\n";
        assert!(list_conflicts(text).unwrap().is_empty());
    }

    #[test]
    fn broken_blocks_are_errors() {
        assert_eq!(parse_conflicts(b"a\n>>>>>>> x\n").unwrap_err().line, 2);
        assert_eq!(parse_conflicts(b"<<<<<<< x\nA\n").unwrap_err().line, 1);
        assert!(parse_conflicts(b"<<<<<<< x\n>>>>>>> y\n").is_err());
        assert!(parse_conflicts(b"<<<<<<< x\n=======\n||||||| y\n>>>>>>> z\n").is_err());
        assert!(parse_conflicts(b"<<<<<<< x\n<<<<<<< y\n").is_err());
    }

    #[test]
    fn crlf_markers_are_seen() {
        let blocks = list_conflicts(b"<<<<<<< a\r\nX\r\n=======\r\nY\r\n>>>>>>> b\r\n").unwrap();
        assert_eq!(blocks[0].ours, vec![b"X\r\n".to_vec()]);
    }
}
