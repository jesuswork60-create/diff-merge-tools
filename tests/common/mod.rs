//! Helpers shared by the integration tests.
#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

static COUNTER: AtomicUsize = AtomicUsize::new(0);

/// A directory under the system temp folder, removed when dropped.
pub struct TempDir(pub PathBuf);

impl TempDir {
    pub fn new() -> TempDir {
        let unique = COUNTER.fetch_add(1, Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!("dmt-test-{}-{unique}", std::process::id()));
        fs::create_dir_all(&path).unwrap();
        TempDir(path)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    pub fn write(&self, name: &str, text: &str) -> PathBuf {
        let path = self.0.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, text).unwrap();
        path
    }

    pub fn read(&self, name: &str) -> String {
        String::from_utf8(fs::read(self.0.join(name)).unwrap()).unwrap()
    }

    pub fn exists(&self, name: &str) -> bool {
        self.0.join(name).exists()
    }

    pub fn arg(&self, name: &str) -> String {
        self.0.join(name).to_str().unwrap().to_string()
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// A small repeatable random number source.
pub struct Random(pub u32);

impl Random {
    pub fn below(&mut self, limit: u32) -> u32 {
        self.0 = self.0.wrapping_mul(1664525).wrapping_add(1013904223);
        (self.0 >> 16) % limit
    }

    /// Some lines drawn from a small vocabulary, so that repeats are common.
    pub fn text(&mut self, max_lines: u32) -> Vec<String> {
        let count = self.below(max_lines + 1);
        (0..count).map(|_| format!("line {}\n", self.below(6))).collect()
    }

    /// A copy of the lines with a few edits.
    pub fn edit(&mut self, lines: &[String]) -> Vec<String> {
        let mut edited = lines.to_vec();
        for _ in 0..self.below(5) {
            let at = self.below(edited.len() as u32 + 1) as usize;
            match self.below(3) {
                0 => edited.insert(at, format!("new {}\n", self.below(100))),
                1 if at < edited.len() => {
                    edited.remove(at);
                }
                _ if at < edited.len() => edited[at] = format!("changed {}\n", self.below(100)),
                _ => {}
            }
        }
        edited
    }
}
