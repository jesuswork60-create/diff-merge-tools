//! A summary of how much a patch changes, in the style of `diffstat`.

use crate::patch::{FilePatch, Patch};

/// Lines added and removed in one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileStat {
    pub path: String,
    pub added: usize,
    pub removed: usize,
}

impl FileStat {
    pub fn of(file: &FilePatch) -> FileStat {
        FileStat {
            path: file.display_path().to_string(),
            added: file.hunks.iter().map(|hunk| hunk.added()).sum(),
            removed: file.hunks.iter().map(|hunk| hunk.removed()).sum(),
        }
    }

    pub fn changed(&self) -> usize {
        self.added + self.removed
    }
}

pub fn stats(patch: &Patch) -> Vec<FileStat> {
    patch.files.iter().map(FileStat::of).collect()
}

fn plural(count: usize, word: &str) -> String {
    format!("{count} {word}{}", if count == 1 { "" } else { "s" })
}

/// One line such as `2 files changed, 3 insertions(+), 1 deletion(-)`.
pub fn summary(stats: &[FileStat]) -> String {
    let added: usize = stats.iter().map(|stat| stat.added).sum();
    let removed: usize = stats.iter().map(|stat| stat.removed).sum();
    let mut text = format!("{} changed", plural(stats.len(), "file"));
    if added > 0 || removed == 0 {
        text.push_str(&format!(", {}(+)", plural(added, "insertion")));
    }
    if removed > 0 {
        text.push_str(&format!(", {}(-)", plural(removed, "deletion")));
    }
    text
}

/// Scales a count so the largest fits the bar, keeping any non-zero count visible.
fn scaled(count: usize, largest: usize, width: usize) -> usize {
    if count == 0 || largest <= width {
        count
    } else {
        (count * width / largest).max(1)
    }
}

/// The whole table, one row per file and the summary at the end.
pub fn render(stats: &[FileStat], bar_width: usize) -> String {
    let name_width = stats.iter().map(|stat| stat.path.chars().count()).max().unwrap_or(0);
    let largest = stats.iter().map(FileStat::changed).max().unwrap_or(0);
    let count_width = largest.to_string().len();
    let mut out = String::new();
    for stat in stats {
        let pluses = scaled(stat.added, largest, bar_width);
        let minuses = scaled(stat.removed, largest, bar_width);
        out.push_str(&format!(
            " {:<name_width$} | {:>count_width$} {}{}\n",
            stat.path,
            stat.changed(),
            "+".repeat(pluses),
            "-".repeat(minuses),
        ));
    }
    out.push(' ');
    out.push_str(&summary(stats));
    out.push('\n');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::parse_patch;

    fn stat(path: &str, added: usize, removed: usize) -> FileStat {
        FileStat { path: path.to_string(), added, removed }
    }

    #[test]
    fn counts_each_file() {
        let patch = parse_patch(b"--- a\n+++ a\n@@ -1,2 +1,3 @@\n k\n-x\n+y\n+z\n--- /dev/null\n+++ b\n@@ -0,0 +1 @@\n+n\n").unwrap();
        assert_eq!(stats(&patch), vec![stat("a", 2, 1), stat("b", 1, 0)]);
    }

    #[test]
    fn a_deleted_file_is_named_by_its_old_path() {
        let patch = parse_patch(b"--- a\n+++ /dev/null\n@@ -1 +0,0 @@\n-x\n").unwrap();
        assert_eq!(stats(&patch), vec![stat("a", 0, 1)]);
    }

    #[test]
    fn summary_wording() {
        assert_eq!(summary(&[stat("a", 1, 1)]), "1 file changed, 1 insertion(+), 1 deletion(-)");
        assert_eq!(summary(&[stat("a", 2, 0), stat("b", 1, 0)]), "2 files changed, 3 insertions(+)");
        assert_eq!(summary(&[stat("a", 0, 2)]), "1 file changed, 2 deletions(-)");
        assert_eq!(summary(&[]), "0 files changed, 0 insertions(+)");
    }

    #[test]
    fn table_lines_up() {
        let table = render(&[stat("short", 2, 1), stat("longer-name", 10, 0)], 40);
        assert_eq!(table, " short       |  3 ++-\n longer-name | 10 ++++++++++\n 2 files changed, 12 insertions(+), 1 deletion(-)\n");
    }

    #[test]
    fn large_counts_are_scaled_and_small_ones_stay_visible() {
        let table = render(&[stat("big", 200, 0), stat("tiny", 1, 1)], 20);
        let rows: Vec<&str> = table.lines().collect();
        assert_eq!(rows[0].matches('+').count(), 20);
        assert!(rows[1].ends_with("+-"));
    }
}
