//! Read instrumentation for [`Archive`](super::archive::Archive) — the number
//! that decides whether a read cache is worth building (#434).
//!
//! Idea: a cache is only a win if the same entry is actually read more than
//! once, and today's read path is already lock-free and positioned
//! (`read_exact_at`), so on a warm OS page cache the saving may be nil. Rather
//! than guess, this counts what a real session does: how many reads happen, how
//! many *distinct* entries they touch, and how many bytes each of those is. The
//! repeat ratio is the cache's best possible hit rate, and the unique byte total
//! is the memory a fully-populated cache would need — both of which #434 wants
//! before any eviction policy is designed.
//!
//! It is **off unless `PK2_READ_STATS` is set**, and the disabled path is a
//! `OnceLock` read plus a branch: no allocation, no lock, no atomics. That
//! matters because #434's rule 3 is that the miss path must stay exactly as fast
//! as it is today, and instrumentation that violated it would measure itself.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use bevy::prelude::info;

/// How many reads between summary lines. Small enough that a run killed mid-load
/// still leaves a number behind, large enough not to spam the log.
const REPORT_EVERY: u64 = 512;

/// Counts of reads against distinct entries.
#[derive(Debug, Default)]
pub struct ReadStats {
    reads: AtomicU64,
    bytes: AtomicU64,
    /// entry -> (times read, size in bytes). Only allocated when enabled.
    seen: Mutex<HashMap<PathBuf, (u64, u64)>>,
}

/// A snapshot of the counters, in the terms #434 asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Summary {
    pub reads: u64,
    pub unique_entries: u64,
    /// Reads that a perfect cache would have served: `reads - unique_entries`.
    pub repeat_reads: u64,
    pub bytes_read: u64,
    /// The bytes a fully-populated cache would hold — the memory budget.
    pub unique_bytes: u64,
}

impl Summary {
    /// The best hit rate any cache could reach on this workload, in percent.
    pub fn best_hit_rate(&self) -> f64 {
        if self.reads == 0 {
            return 0.0;
        }
        self.repeat_reads as f64 * 100.0 / self.reads as f64
    }
}

impl ReadStats {
    pub fn record(&self, path: &Path, size: u64) {
        let reads = self.reads.fetch_add(1, Ordering::Relaxed) + 1;
        self.bytes.fetch_add(size, Ordering::Relaxed);
        if let Ok(mut seen) = self.seen.lock() {
            let entry = seen.entry(path.to_path_buf()).or_insert((0, size));
            entry.0 += 1;
        }
        if reads % REPORT_EVERY == 0 {
            self.report();
        }
    }

    pub fn summary(&self) -> Summary {
        let reads = self.reads.load(Ordering::Relaxed);
        let (unique_entries, unique_bytes) = match self.seen.lock() {
            Ok(seen) => (
                seen.len() as u64,
                seen.values().map(|(_, size)| *size).sum::<u64>(),
            ),
            Err(_) => (0, 0),
        };
        Summary {
            reads,
            unique_entries,
            repeat_reads: reads.saturating_sub(unique_entries),
            bytes_read: self.bytes.load(Ordering::Relaxed),
            unique_bytes,
        }
    }

    /// The `n` entries read most often, worst first.
    ///
    /// A high repeat ratio has two very different causes — many consumers
    /// legitimately sharing one texture, or one consumer re-reading the same
    /// entry in a loop — and only the offender list tells them apart. A cache
    /// fixes the first; the second is a bug a cache would merely hide.
    pub fn top_repeats(&self, n: usize) -> Vec<(PathBuf, u64)> {
        let Ok(seen) = self.seen.lock() else {
            return Vec::new();
        };
        let mut rows: Vec<(PathBuf, u64)> = seen
            .iter()
            .filter(|(_, (count, _))| *count > 1)
            .map(|(path, (count, _))| (path.clone(), *count))
            .collect();
        rows.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        rows.truncate(n);
        rows
    }

    fn report(&self) {
        let s = self.summary();
        let worst: Vec<String> = self
            .top_repeats(5)
            .into_iter()
            .map(|(path, count)| format!("{}x {}", count, path.display()))
            .collect();
        info!("pk2 read stats: most-read entries: {}", worst.join(" · "));
        info!(
            "pk2 read stats: {} reads over {} unique entries ({} repeats, best possible hit rate \
             {:.1}%), {:.1} MiB read, {:.1} MiB unique",
            s.reads,
            s.unique_entries,
            s.repeat_reads,
            s.best_hit_rate(),
            s.bytes_read as f64 / (1024.0 * 1024.0),
            s.unique_bytes as f64 / (1024.0 * 1024.0),
        );
    }
}

/// `Some` only when `PK2_READ_STATS` is set to something other than `0`/empty.
fn stats() -> Option<&'static ReadStats> {
    static STATS: OnceLock<Option<ReadStats>> = OnceLock::new();
    STATS
        .get_or_init(|| {
            let enabled = std::env::var("PK2_READ_STATS")
                .map(|value| !matches!(value.trim(), "" | "0" | "false" | "no"))
                .unwrap_or(false);
            if enabled {
                info!(
                    "pk2 read stats: enabled (PK2_READ_STATS); summary every {REPORT_EVERY} reads"
                );
                Some(ReadStats::default())
            } else {
                None
            }
        })
        .as_ref()
}

/// Record one entry read. A no-op unless `PK2_READ_STATS` is set.
pub fn record(path: &Path, size: u64) {
    if let Some(stats) = stats() {
        stats.record(path, size);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #434 step 1: the number that decides the cache is "how many reads would a
    /// cache have served", so repeats — not reads — are what has to be counted,
    /// and the unique byte total is the memory such a cache would need.
    #[test]
    fn repeats_and_unique_bytes_are_what_a_cache_would_save() {
        let stats = ReadStats::default();
        stats.record(Path::new("res/a.ddj"), 100);
        stats.record(Path::new("res/b.ddj"), 200);
        stats.record(Path::new("res/a.ddj"), 100);
        stats.record(Path::new("res/a.ddj"), 100);

        let s = stats.summary();
        assert_eq!(s.reads, 4);
        assert_eq!(s.unique_entries, 2);
        assert_eq!(s.repeat_reads, 2, "two of the four reads were repeats");
        assert_eq!(s.bytes_read, 500);
        assert_eq!(s.unique_bytes, 300, "a full cache would hold 300 bytes");
        assert!((s.best_hit_rate() - 50.0).abs() < f64::EPSILON);

        // the offender list is what separates "shared asset" from "re-read in a
        // loop", so it is ordered worst-first and drops the read-once entries
        assert_eq!(
            stats.top_repeats(5),
            vec![(PathBuf::from("res/a.ddj"), 3)],
            "b.ddj was read once and is not a repeat"
        );
    }

    /// An empty run must report 0%, not divide by zero — the summary is printed
    /// from a background log line that can fire before anything is loaded.
    #[test]
    fn an_empty_run_reports_no_hit_rate_rather_than_dividing_by_zero() {
        let s = ReadStats::default().summary();
        assert_eq!(s.reads, 0);
        assert_eq!(s.repeat_reads, 0);
        assert_eq!(s.best_hit_rate(), 0.0);
    }

    /// The instrumentation must be inert by default: a build that measured
    /// itself would answer #434 with its own overhead. `PK2_READ_STATS` is unset
    /// in the gate, so `record` here must touch nothing.
    #[test]
    fn the_recorder_is_off_unless_the_env_var_asks_for_it() {
        if std::env::var("PK2_READ_STATS").is_ok() {
            return; // a developer running with it on: nothing to assert
        }
        assert!(stats().is_none());
        record(Path::new("res/a.ddj"), 1); // must not panic and must not count
        assert!(stats().is_none());
    }
}
