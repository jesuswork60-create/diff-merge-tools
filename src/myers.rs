//! The greedy O(ND) diff algorithm from Myers' 1986 paper.
//!
//! It keeps a copy of the frontier for every value of D so it can walk back along the
//! path, which makes memory grow with the square of the number of differences. Common
//! prefixes and suffixes are removed first, since most real edits are small and local.

/// What a [`Span`] does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Equal,
    Delete,
    Insert,
}

/// A run of one operation. Positions are indexes into the two input sequences; a delete
/// consumes only old items and an insert only new ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub op: Op,
    pub old_start: usize,
    pub new_start: usize,
    pub len: usize,
}

/// Computes a shortest edit script. Between two equal runs, deletes always come before inserts.
pub fn diff<T: PartialEq>(old: &[T], new: &[T]) -> Vec<Span> {
    let prefix = old.iter().zip(new).take_while(|(a, b)| a == b).count();
    let suffix = old[prefix..]
        .iter()
        .rev()
        .zip(new[prefix..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    let middle = shortest_edit(&old[prefix..old.len() - suffix], &new[prefix..new.len() - suffix]);
    let mut builder = Builder::default();
    builder.equal(prefix);
    for op in middle {
        match op {
            Op::Equal => builder.equal(1),
            Op::Delete => builder.deleted += 1,
            Op::Insert => builder.inserted += 1,
        }
    }
    builder.equal(suffix);
    builder.flush_changes();
    builder.spans
}

/// Turns single steps into spans, holding back changes so deletes can be put first.
#[derive(Default)]
struct Builder {
    spans: Vec<Span>,
    old_at: usize,
    new_at: usize,
    deleted: usize,
    inserted: usize,
}

impl Builder {
    fn flush_changes(&mut self) {
        if self.deleted > 0 {
            self.spans.push(Span { op: Op::Delete, old_start: self.old_at, new_start: self.new_at, len: self.deleted });
            self.old_at += self.deleted;
            self.deleted = 0;
        }
        if self.inserted > 0 {
            self.spans.push(Span { op: Op::Insert, old_start: self.old_at, new_start: self.new_at, len: self.inserted });
            self.new_at += self.inserted;
            self.inserted = 0;
        }
    }

    fn equal(&mut self, len: usize) {
        if len == 0 {
            return;
        }
        self.flush_changes();
        match self.spans.last_mut() {
            Some(last) if last.op == Op::Equal => last.len += len,
            _ => self.spans.push(Span { op: Op::Equal, old_start: self.old_at, new_start: self.new_at, len }),
        }
        self.old_at += len;
        self.new_at += len;
    }
}

fn shortest_edit<T: PartialEq>(a: &[T], b: &[T]) -> Vec<Op> {
    let n = a.len() as isize;
    let m = b.len() as isize;
    let max = n + m;
    if max == 0 {
        return Vec::new();
    }
    let offset = max + 1;
    let mut frontier = vec![0isize; (2 * max + 3) as usize];
    let mut trace: Vec<Vec<isize>> = Vec::new();
    'search: for d in 0..=max {
        trace.push(frontier[(offset - d - 1) as usize..=(offset + d + 1) as usize].to_vec());
        let mut k = -d;
        while k <= d {
            let at = (offset + k) as usize;
            let mut x = if k == -d || (k != d && frontier[at - 1] < frontier[at + 1]) {
                frontier[at + 1]
            } else {
                frontier[at - 1] + 1
            };
            let mut y = x - k;
            while x < n && y < m && a[x as usize] == b[y as usize] {
                x += 1;
                y += 1;
            }
            frontier[at] = x;
            if x >= n && y >= m {
                break 'search;
            }
            k += 2;
        }
    }
    walk_back(&trace, n, m)
}

fn walk_back(trace: &[Vec<isize>], n: isize, m: isize) -> Vec<Op> {
    let mut ops = Vec::new();
    let (mut x, mut y) = (n, m);
    for (d, snapshot) in trace.iter().enumerate().rev() {
        let d = d as isize;
        let reach = |k: isize| snapshot[(k + d + 1) as usize];
        let k = x - y;
        let previous_k = if k == -d || (k != d && reach(k - 1) < reach(k + 1)) { k + 1 } else { k - 1 };
        let previous_x = reach(previous_k);
        let previous_y = previous_x - previous_k;
        while x > previous_x && y > previous_y {
            ops.push(Op::Equal);
            x -= 1;
            y -= 1;
        }
        if d > 0 {
            ops.push(if x == previous_x { Op::Insert } else { Op::Delete });
        }
        x = previous_x;
        y = previous_y;
    }
    ops.reverse();
    ops
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rebuild(old: &[char], new: &[char], spans: &[Span]) -> (Vec<char>, Vec<char>) {
        let mut from_old = Vec::new();
        let mut to_new = Vec::new();
        for span in spans {
            match span.op {
                Op::Equal => {
                    assert_eq!(old[span.old_start..span.old_start + span.len], new[span.new_start..span.new_start + span.len]);
                    from_old.extend_from_slice(&old[span.old_start..span.old_start + span.len]);
                    to_new.extend_from_slice(&new[span.new_start..span.new_start + span.len]);
                }
                Op::Delete => from_old.extend_from_slice(&old[span.old_start..span.old_start + span.len]),
                Op::Insert => to_new.extend_from_slice(&new[span.new_start..span.new_start + span.len]),
            }
        }
        (from_old, to_new)
    }

    fn cost(spans: &[Span]) -> usize {
        spans.iter().filter(|span| span.op != Op::Equal).map(|span| span.len).sum()
    }

    fn chars(text: &str) -> Vec<char> {
        text.chars().collect()
    }

    fn lcs_edit_distance(a: &[char], b: &[char]) -> usize {
        let mut table = vec![vec![0usize; b.len() + 1]; a.len() + 1];
        for i in 0..a.len() {
            for j in 0..b.len() {
                table[i + 1][j + 1] = if a[i] == b[j] { table[i][j] + 1 } else { table[i + 1][j].max(table[i][j + 1]) };
            }
        }
        a.len() + b.len() - 2 * table[a.len()][b.len()]
    }

    #[test]
    fn identical_inputs_are_one_equal_span() {
        let a = chars("abc");
        assert_eq!(diff(&a, &a), vec![Span { op: Op::Equal, old_start: 0, new_start: 0, len: 3 }]);
    }

    #[test]
    fn both_empty_gives_nothing() {
        assert!(diff::<char>(&[], &[]).is_empty());
    }

    #[test]
    fn one_side_empty() {
        let a = chars("abc");
        assert_eq!(diff(&a, &[]), vec![Span { op: Op::Delete, old_start: 0, new_start: 0, len: 3 }]);
        assert_eq!(diff(&[], &a), vec![Span { op: Op::Insert, old_start: 0, new_start: 0, len: 3 }]);
    }

    #[test]
    fn the_example_from_the_paper() {
        let (a, b) = (chars("abcabba"), chars("cbabac"));
        let spans = diff(&a, &b);
        assert_eq!(cost(&spans), 5);
        assert_eq!(rebuild(&a, &b, &spans), (a, b));
    }

    #[test]
    fn deletes_come_before_inserts() {
        let spans = diff(&chars("axb"), &chars("ayb"));
        let ops: Vec<Op> = spans.iter().map(|span| span.op).collect();
        assert_eq!(ops, vec![Op::Equal, Op::Delete, Op::Insert, Op::Equal]);
    }

    #[test]
    fn matches_dynamic_programming_on_pseudo_random_input() {
        let mut state = 7u32;
        let mut next = move |limit: u32| {
            state = state.wrapping_mul(1664525).wrapping_add(1013904223);
            (state >> 16) % limit
        };
        for _ in 0..300 {
            let a: Vec<char> = (0..next(25)).map(|_| (b'a' + next(4) as u8) as char).collect();
            let b: Vec<char> = (0..next(25)).map(|_| (b'a' + next(4) as u8) as char).collect();
            let spans = diff(&a, &b);
            assert_eq!(cost(&spans), lcs_edit_distance(&a, &b), "{a:?} {b:?}");
            assert_eq!(rebuild(&a, &b, &spans), (a, b));
        }
    }
}
