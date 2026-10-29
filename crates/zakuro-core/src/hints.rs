//! where a title ran in the interpreter while it had a recompiled library,
//! code 3dsrecomp did not find. the addresses go in a file next to the
//! library, which the next build takes as functions, and how much the
//! interpreter spent at each place goes beside it, for the profile of what
//! is worth lifting or overriding next.

use std::collections::BTreeMap;
use std::ops::Range;
use std::path::{Path, PathBuf};

/// how often the addresses go to the file, in frames, a minute.
pub const SAVE_EVERY: u64 = 3600;

pub struct Hints {
    path: PathBuf,
    /// the executable's code, the only place addresses are the same every
    /// run.
    text: Range<u32>,
    /// how many interpreted runs started at each address, odd for Thumb.
    seen: BTreeMap<u32, u64>,
    /// every instruction the interpreter ran at an address the library has
    /// no entry for, the interpreter traffic by place.
    traffic: BTreeMap<u32, u64>,
    saved: usize,
    /// the traffic total at the last profile write, so an unchanged run
    /// does not rewrite the file.
    saved_traffic: u64,
    /// the instruction before was interpreted too.
    interpreting: bool,
}

impl Hints {
    /// the hints for a library, with those an earlier run wrote down.
    pub fn new(library: &Path, text: Range<u32>) -> Hints {
        let path = library.with_extension("hints");
        let seen: BTreeMap<u32, u64> = std::fs::read_to_string(&path)
            .unwrap_or_default()
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .filter_map(|line| u32::from_str_radix(line.trim_start_matches("0x"), 16).ok())
            .map(|address| (address, 0))
            .collect();
        let saved = seen.len();
        Hints { path, text, seen, traffic: BTreeMap::new(), saved, saved_traffic: 0, interpreting: false }
    }

    /// the library ran some code.
    pub fn library_ran(&mut self) {
        self.interpreting = false;
    }

    /// the library had code but not budget for the whole block, so the
    /// interpreter goes on through it. what it runs until the library takes
    /// over again is known code, not missing code.
    pub fn library_declined(&mut self) {
        self.interpreting = true;
    }

    /// the interpreter ran an instruction at pc that the library has no
    /// entry for. where that follows the library's code, missing code
    /// starts, what it goes on to call the next build finds from there;
    /// every such instruction counts toward the traffic.
    pub fn interpreted(&mut self, pc: u32, thumb: bool) {
        let at = pc & !1;
        *self.traffic.entry(at | thumb as u32).or_insert(0) += 1;
        if !self.interpreting && self.text.contains(&at) {
            *self.seen.entry(at | thumb as u32).or_insert(0) += 1;
        }
        self.interpreting = true;
    }

    /// the traffic by place, for putting beside the code's own fallbacks.
    pub fn traffic(&self) -> BTreeMap<u32, u64> {
        self.traffic.clone()
    }

    /// writes the hints file when there is anything new, and the traffic
    /// profile (the code's own fallbacks beside the interpreter's) when it
    /// moved.
    pub fn save_with_fallbacks(&mut self, fallbacks: &BTreeMap<u32, u64>) {
        if self.seen.len() != self.saved {
            let mut text = String::from("# where Zakuro interpreted this title's code, odd for Thumb, which 3dsrecomp build recompiles\n");
            for address in self.seen.keys() {
                text.push_str(&format!("{address:08X}\n"));
            }
            let partial = self.path.with_extension("hints.new");
            match std::fs::write(&partial, text).and_then(|()| std::fs::rename(&partial, &self.path)) {
                Ok(()) => self.saved = self.seen.len(),
                Err(error) => log::warn!("could not write {}, {error}", self.path.display()),
            }
        }
        let total: u64 = self.traffic.values().sum();
        if total != self.saved_traffic {
            let mut rows: Vec<(u64, u32, &str)> =
                self.traffic.iter().map(|(address, count)| (*count, *address, "interpreter")).collect();
            for (address, count) in fallbacks {
                rows.push((*count, *address, "fallback"));
            }
            rows.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
            let mut text = String::from("# interpreter traffic by place: count, address, kind (odd for Thumb)\n");
            for (count, address, kind) in &rows {
                text.push_str(&format!("{count}\t{address:08X}\t{kind}\n"));
            }
            let path = PathBuf::from(format!("{}.profile", self.path.display()));
            let partial = path.with_extension("new");
            match std::fs::write(&partial, text).and_then(|()| std::fs::rename(&partial, &path)) {
                Ok(()) => self.saved_traffic = total,
                Err(error) => log::warn!("could not write {}, {error}", path.display()),
            }
        }
    }

    /// the hints alone, when there are no fallback counts to put beside.
    pub fn save(&mut self) {
        self.save_with_fallbacks(&BTreeMap::new());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_runs_after_the_library_declines_is_no_hint() {
        let library = std::env::temp_dir().join(format!("zakuro-hints-{}", std::process::id())).join("lib.so");
        let mut hints = Hints::new(&library, 0x0010_0000..0x0020_0000);
        // the rest of a block the budget did not cover
        hints.library_declined();
        hints.interpreted(0x0010_0004, false);
        hints.interpreted(0x0010_0008, false);
        assert!(hints.seen.is_empty());
        // but the traffic counts every instruction all the same
        assert_eq!(hints.traffic().get(&0x0010_0004), Some(&1));
        assert_eq!(hints.traffic().get(&0x0010_0008), Some(&1));
        // code the library does not have, right after it ran
        hints.library_ran();
        hints.interpreted(0x0010_0100, true);
        assert_eq!(hints.seen.keys().copied().collect::<Vec<_>>(), vec![0x0010_0101]);
        // the entry ran twice, so its count says so
        hints.library_ran();
        hints.interpreted(0x0010_0100, true);
        assert_eq!(hints.seen.get(&0x0010_0101), Some(&2));
    }

    #[test]
    fn loaded_hints_are_not_rewritten_until_new_ones_come() {
        let dir = std::env::temp_dir().join(format!("zakuro-hints-loaded-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let library = dir.join("lib.so");
        std::fs::write(library.with_extension("hints"), "# test\n00100101\n00100201\n").unwrap();
        let mut hints = Hints::new(&library, 0x0010_0000..0x0020_0000);
        assert_eq!(hints.seen.len(), 2);
        hints.library_ran();
        hints.interpreted(0x0010_0100, true);
        // same addresses as loaded: the file stays as it was
        hints.save();
        let text = std::fs::read_to_string(library.with_extension("hints")).unwrap();
        assert_eq!(text.lines().filter(|line| !line.starts_with('#')).count(), 2);
        std::fs::remove_dir_all(&dir).ok();
    }
}
