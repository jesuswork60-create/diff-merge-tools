# diff-merge-tools

A line diff, a patch applier and a three-way merge, written in Rust with no dependencies.
It comes as a library (`diffmerge`) and a command line program (`dmt`), which covers roughly
the everyday parts of `diff -u`, `patch`, `diff3 -m` and `git merge-file`.

Everything works on bytes, so files in any encoding pass through untouched, and each line keeps
its own terminator. That also means a file without a final newline is handled properly, using the
usual `\ No newline at end of file` marker.

## Install

You need a stable Rust toolchain.

    cargo build --release
    ./target/release/dmt --help

## Usage

Make a patch and apply it:

    $ printf 'alpha\nbeta\ngamma\ndelta\n' > greeting.txt
    $ printf 'alpha\nBETA\ngamma\ndelta\nepsilon\n' > new.txt
    $ dmt diff greeting.txt new.txt > change.patch
    $ cat change.patch
    --- greeting.txt
    +++ new.txt
    @@ -1,4 +1,5 @@
     alpha
    -beta
    +BETA
     gamma
     delta
    +epsilon
    $ dmt stat change.patch
     new.txt | 3 ++-
     1 file changed, 2 insertions(+), 1 deletion(-)

`dmt patch` takes the target path from the patch headers, so this one would need its headers to
name `greeting.txt` to change that file. Hunks that no longer line up are searched for nearby, with up to
two context lines ignored at each end. Those that can't be placed go in a `.rej` file next to the
target, and the exit code is 1.

Merge two sets of changes to the same file:

    $ printf 'a\nb\nc\nd\ne\n' > base.txt
    $ printf 'a\nB ours\nc\nd\ne\n' > ours.txt
    $ printf 'a\nB theirs\nc\nd\nE\n' > theirs.txt
    $ dmt merge ours.txt base.txt theirs.txt
    a
    <<<<<<< ours.txt
    B ours
    =======
    B theirs
    >>>>>>> theirs.txt
    c
    d
    E

Then `dmt conflicts FILE` lists the blocks left in a file, and `dmt resolve --ours FILE` settles
them (also `--theirs`, `--base` and `--union`). `dmt --help` has the full list of options.

Exit codes: 0 when there is nothing to report, 1 when files differ, hunks are rejected or conflicts
remain, and 2 for bad usage or an unreadable file.

From Rust:

```rust
use diffmerge::diff::{diff_texts, DiffOptions};
use diffmerge::merge::{merge, MergeOptions};

let hunks = diff_texts(b"one\ntwo\n", b"one\n2\n", &DiffOptions::default());
assert_eq!(hunks.len(), 1);

let merged = merge(b"x\n", b"o\n", b"o\n", &MergeOptions::default());
assert!(merged.conflicts.is_empty());
```

## How it is laid out

`myers` holds the greedy O(ND) algorithm. `diff` turns its output into hunks, `patch` is the data
model and writer, and `parse` reads patches back. `apply` places hunks on a file in memory, and
`workspace` does the file system side, with an overlay so that `--atomic` and `--dry-run` write
nothing. `merge` is diff3 in the style of Khanna, Myers and Pierce, and `conflicts` reads the
markers it writes. `word`, `stat` and `tree` are the smaller features behind `dmt diff`.

## Tests

    cargo test

There are unit tests in each module and integration tests in `tests/`, including randomized round
trips with a fixed seed and a comparison of the diff against a plain dynamic programming LCS.

## Known limitations

- The Myers trace keeps a copy of the frontier per edit, so memory grows with the square of the edit
  distance. Two large, completely different files are slow and use a lot of memory; there is no
  linear-space variant and no heuristic cutoff.
- Merge compares lines exactly and has no whitespace option. Edits on adjacent lines count as a
  conflict even when they don't touch the same line.
- Conflict markers are always seven characters wide, and there is no `zdiff3` style that trims
  matching lines from a conflict.
- `patch` handles text only. It records `Binary files differ` as a note and gives an error on `GIT
  binary patch` sections. Git's extended headers are skipped, so renames, copies and mode changes
  are not applied.
- The directory diff skips symlinks, reports binary and empty files only as notes, and matches
  `--exclude` against exact names, not patterns.
- `workspace` keeps paths inside the root by rejecting absolute paths and `..`, but it does not
  check for symlinks inside the root that lead out of it. Directories left empty after a delete stay
  behind.
- Option values must be separate arguments (`-p 1`, not `-p1`), and the subcommands don't have
  their own `--help`.
