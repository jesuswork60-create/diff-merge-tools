//! Properties that tie the parts together, checked over many generated inputs.

mod common;

use common::Random;
use diffmerge::apply::{apply_hunks, ApplyOptions};
use diffmerge::conflicts::{list_conflicts, resolve, Resolution};
use diffmerge::diff::{diff_file, DiffOptions};
use diffmerge::merge::{merge, MergeOptions};
use diffmerge::parse::parse_patch;
use diffmerge::patch::{format_patch, Patch};

const ROUNDS: u32 = 400;

#[test]
fn a_diff_applies_forwards_and_backwards() {
    let mut random = Random(11);
    for _ in 0..ROUNDS {
        let old = random.text(30).concat();
        let new = random.text(30).concat();
        let context = random.below(4) as usize;
        let file = diff_file(Some("x"), old.as_bytes(), Some("x"), new.as_bytes(), &DiffOptions { context, ..DiffOptions::default() });
        let forward = apply_hunks(old.as_bytes(), &file.hunks, &ApplyOptions::default());
        assert!(forward.is_clean(), "{old:?} -> {new:?}");
        assert_eq!(forward.text, new.as_bytes(), "{old:?} -> {new:?}");
        let backward = file.reversed();
        let back = apply_hunks(new.as_bytes(), &backward.hunks, &ApplyOptions::default());
        assert!(back.is_clean());
        assert_eq!(back.text, old.as_bytes());
    }
}

#[test]
fn a_formatted_diff_parses_back_to_the_same_thing() {
    let mut random = Random(23);
    for _ in 0..ROUNDS {
        let old = random.text(25).concat();
        let new = random.text(25).concat();
        // Dropping the final newline on some inputs exercises the marker line.
        let old = if random.below(4) == 0 { old.trim_end_matches('\n').to_string() } else { old };
        let new = if random.below(4) == 0 { new.trim_end_matches('\n').to_string() } else { new };
        let file = diff_file(Some("a/x"), old.as_bytes(), Some("b/x"), new.as_bytes(), &DiffOptions::default());
        if file.hunks.is_empty() {
            continue;
        }
        let patch = Patch { files: vec![file], notes: Vec::new() };
        let text = format_patch(&patch);
        let parsed = parse_patch(&text).unwrap_or_else(|error| panic!("{error}: {}", String::from_utf8_lossy(&text)));
        assert_eq!(parsed, patch);
        let applied = apply_hunks(old.as_bytes(), &parsed.files[0].hunks, &ApplyOptions::default());
        assert_eq!(applied.text, new.as_bytes());
    }
}

#[test]
fn a_diff_still_applies_after_lines_are_added_at_the_top() {
    let mut random = Random(31);
    for _ in 0..ROUNDS {
        // Lines the edits never contain, so the moved hunk can only land in one place.
        let old: Vec<String> = (0..20).map(|n| format!("unique {n}\n")).collect();
        let new = random.edit(&old);
        let file = diff_file(Some("x"), old.concat().as_bytes(), Some("x"), new.concat().as_bytes(), &DiffOptions::default());
        let moved = format!("added\nadded too\n{}", old.concat());
        let expected = format!("added\nadded too\n{}", new.concat());
        let report = apply_hunks(moved.as_bytes(), &file.hunks, &ApplyOptions::default());
        assert!(report.is_clean());
        assert_eq!(report.text, expected.as_bytes());
    }
}

#[test]
fn merging_with_an_unchanged_side_gives_the_other() {
    let mut random = Random(41);
    for _ in 0..ROUNDS {
        let base = random.text(25);
        let edited = random.edit(&base);
        let (base, edited) = (base.concat(), edited.concat());
        let one = merge(base.as_bytes(), edited.as_bytes(), base.as_bytes(), &MergeOptions::default());
        assert!(one.is_clean());
        assert_eq!(one.text, edited.as_bytes());
        let other = merge(base.as_bytes(), base.as_bytes(), edited.as_bytes(), &MergeOptions::default());
        assert!(other.is_clean());
        assert_eq!(other.text, edited.as_bytes());
    }
}

#[test]
fn merging_identical_sides_is_clean() {
    let mut random = Random(53);
    for _ in 0..ROUNDS {
        let base = random.text(25).concat();
        let edited = random.text(25).concat();
        let result = merge(base.as_bytes(), edited.as_bytes(), edited.as_bytes(), &MergeOptions::default());
        assert!(result.is_clean());
        assert_eq!(result.text, edited.as_bytes());
    }
}

#[test]
fn a_merge_is_symmetric_up_to_the_markers() {
    let mut random = Random(61);
    for _ in 0..ROUNDS {
        let base = random.text(25);
        let (ours, theirs) = (random.edit(&base).concat(), random.edit(&base).concat());
        let base = base.concat();
        let one = merge(base.as_bytes(), ours.as_bytes(), theirs.as_bytes(), &MergeOptions::default());
        let two = merge(base.as_bytes(), theirs.as_bytes(), ours.as_bytes(), &MergeOptions::default());
        assert_eq!(one.conflicts.len(), two.conflicts.len());
        if one.is_clean() {
            assert_eq!(one.text, two.text);
        }
    }
}

#[test]
fn conflicts_written_by_a_merge_can_be_read_and_settled() {
    let mut random = Random(71);
    let mut seen = 0;
    for _ in 0..ROUNDS {
        let base = random.text(25);
        let (ours, theirs) = (random.edit(&base).concat(), random.edit(&base).concat());
        let base = base.concat();
        let result = merge(base.as_bytes(), ours.as_bytes(), theirs.as_bytes(), &MergeOptions::default());
        let blocks = list_conflicts(&result.text).unwrap();
        assert_eq!(blocks.len(), result.conflicts.len());
        for (block, spot) in blocks.iter().zip(&result.conflicts) {
            assert_eq!(block.line, spot.line);
            assert_eq!(block.ours.len(), spot.ours_lines);
            assert_eq!(block.theirs.len(), spot.theirs_lines);
        }
        seen += blocks.len();
        let settled = resolve(&result.text, Resolution::Ours).unwrap();
        let favored = merge(base.as_bytes(), ours.as_bytes(), theirs.as_bytes(), &MergeOptions { favor: diffmerge::merge::Favor::Ours, ..MergeOptions::default() });
        assert_eq!(settled, favored.text);
    }
    assert!(seen > 0, "the generated input should produce some conflicts");
}
