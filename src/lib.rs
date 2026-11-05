//! Line diffs, unified patches and three-way merges.
//!
//! Everything works on bytes rather than strings, so files in any encoding go through
//! unchanged, and a line always keeps its own terminator.
//!
//! ```
//! use diffmerge::diff::{diff_texts, DiffOptions};
//! use diffmerge::merge::{merge, MergeOptions};
//!
//! let hunks = diff_texts(b"one\ntwo\n", b"one\n2\n", &DiffOptions::default());
//! assert_eq!(hunks.len(), 1);
//!
//! let merged = merge(b"x\n", b"o\n", b"o\n", &MergeOptions::default());
//! assert!(merged.conflicts.is_empty());
//! ```

pub mod apply;
pub mod cli;
pub mod conflicts;
pub mod diff;
pub mod lines;
pub mod merge;
pub mod myers;
pub mod parse;
pub mod patch;
pub mod stat;
pub mod tree;
pub mod word;
pub mod workspace;
