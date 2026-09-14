//! Rank the systems in a Bevy chrome trace by self time.
//!
//! Idea: a `--features profile-chrome` run writes `trace-<nanos>.json`, a Chrome
//! Trace Event file. Perfetto renders it beautifully but answers "what happened
//! at this instant"; the performance question is "where does the frame go on
//! average", which is a different reduction: per-span **self** time (total minus
//! the time attributed to nested spans), summed over the capture.
//!
//! Self time rather than wall time is the whole point. Bevy's spans nest --
//! `schedule` contains `system`, which contains whatever we instrument inside it
//! -- so wall time double-counts and puts the root schedule on top every time,
//! which says nothing. Self time attributes each microsecond to exactly one span.
//!
//! Everything here streams. The writer emits tens of MB per second, so a session
//! long enough to be interesting is measured in gigabytes -- an 8.9 GB capture is
//! what prompted this design. Nothing holds the file, or the event list, in
//! memory: lines are parsed one at a time into a borrowed struct, folded into
//! per-thread stacks, and dropped.
//!
//! A long capture also spans more than one regime -- login, terrain streaming,
//! running around, then standing still. Aggregating all of it answers a question
//! nobody asked, so `--last-secs` restricts the report to the tail, where the
//! camera was presumably parked. That is kept as a bounded ring of per-second
//! buckets rather than a second pass over the file.
//!
//! Usage:
//!   cargo run --release -p tools --bin trace_summary -- trace.json [--top 30] [--last-secs 60]

use std::borrow::Cow;
use std::collections::HashMap;
use std::collections::VecDeque;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::process::ExitCode;

use serde::Deserialize;

/// One span name's accumulated cost.
#[derive(Default, Clone)]
struct Span {
    /// Time with nested spans subtracted -- what this span itself cost.
    self_us: f64,
    /// Time including nested spans. A large gap between the two is itself
    /// informative: the cost is in children, not here.
    total_us: f64,
    calls: u64,
}

impl Span {
    fn add(&mut self, total: f64, child: f64) {
        self.total_us += total;
        self.self_us += (total - child).max(0.0);
        self.calls += 1;
    }
    fn merge(&mut self, other: &Span) {
        self.self_us += other.self_us;
        self.total_us += other.total_us;
        self.calls += other.calls;
    }
}

/// A span currently open on some thread.
struct Open {
    name: String,
    start_us: f64,
    /// Time consumed by spans that opened and closed inside this one.
    child_us: f64,
}

/// Only the fields the reduction needs. Borrowed from the line buffer, so a
/// parsed event allocates nothing unless its name carries a JSON escape.
#[derive(Deserialize)]
struct RawEvent<'a> {
    ph: Cow<'a, str>,
    #[serde(default)]
    ts: f64,
    #[serde(default)]
    pid: i64,
    #[serde(default)]
    tid: i64,
    #[serde(default, borrow)]
    name: Option<Cow<'a, str>>,
    #[serde(default)]
    dur: Option<f64>,
}

type Aggregate = HashMap<String, Span>;

/// Folds events into per-thread stacks, keeping a whole-session aggregate and a
/// bounded ring of recent one-second buckets.
struct Reducer {
    all: Aggregate,
    stacks: HashMap<(i64, i64), Vec<Open>>,
    /// (bucket index, aggregate) for the most recent `window` seconds.
    recent: VecDeque<(i64, Aggregate)>,
    window_secs: Option<i64>,
    first_ts: Option<f64>,
    last_ts: f64,
    /// Events whose `E` had no matching `B` -- a span that began before the
    /// capture, or a thread whose ordering assumption broke.
    unmatched: u64,
}

impl Reducer {
    fn new(window_secs: Option<i64>) -> Self {
        Self {
            all: Aggregate::new(),
            stacks: HashMap::new(),
            recent: VecDeque::new(),
            window_secs,
            first_ts: None,
            last_ts: 0.0,
            unmatched: 0,
        }
    }

    fn record(&mut self, name: &str, ts: f64, total: f64, child: f64) {
        self.all
            .entry(name.to_string())
            .or_default()
            .add(total, child);
        let Some(window) = self.window_secs else {
            return;
        };
        // Chrome timestamps are microseconds.
        let bucket = (ts / 1_000_000.0) as i64;
        if self.recent.back().map(|(b, _)| *b) != Some(bucket) {
            self.recent.push_back((bucket, Aggregate::new()));
            while self
                .recent
                .front()
                .is_some_and(|(b, _)| bucket - *b >= window)
            {
                self.recent.pop_front();
            }
        }
        if let Some((_, agg)) = self.recent.back_mut() {
            agg.entry(name.to_string()).or_default().add(total, child);
        }
    }

    fn push(&mut self, event: RawEvent) {
        let phase = event.ph.chars().next().unwrap_or('?');
        if !matches!(phase, 'B' | 'E' | 'X') {
            return;
        }
        self.first_ts.get_or_insert(event.ts);
        self.last_ts = self.last_ts.max(event.ts);
        let key = (event.pid, event.tid);

        match phase {
            'B' => {
                let name = event
                    .name
                    .unwrap_or(Cow::Borrowed("<unnamed>"))
                    .into_owned();
                self.stacks.entry(key).or_default().push(Open {
                    name,
                    start_us: event.ts,
                    child_us: 0.0,
                });
            }
            'E' => {
                let Some(open) = self.stacks.get_mut(&key).and_then(Vec::pop) else {
                    self.unmatched += 1;
                    return;
                };
                let total = event.ts - open.start_us;
                self.record(&open.name, event.ts, total, open.child_us);
                if let Some(parent) = self.stacks.get_mut(&key).and_then(|s| s.last_mut()) {
                    parent.child_us += total;
                }
            }
            // Complete events carry their own duration: a B/E pair already closed.
            'X' => {
                let dur = event.dur.unwrap_or(0.0);
                let name = event.name.unwrap_or(Cow::Borrowed("<unnamed>"));
                self.record(&name, event.ts, dur, 0.0);
                if let Some(parent) = self.stacks.get_mut(&key).and_then(|s| s.last_mut()) {
                    parent.child_us += dur;
                }
            }
            _ => {}
        }
    }

    fn recent_aggregate(&self) -> Aggregate {
        let mut merged = Aggregate::new();
        for (_, agg) in &self.recent {
            for (name, span) in agg {
                merged.entry(name.clone()).or_default().merge(span);
            }
        }
        merged
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut path: Option<String> = None;
    let mut top = 40usize;
    let mut last_secs: Option<i64> = None;

    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--top" => match it.next().and_then(|v| v.parse().ok()) {
                Some(n) => top = n,
                None => return fail("--top needs a number"),
            },
            "--last-secs" => match it.next().and_then(|v| v.parse().ok()) {
                Some(n) => last_secs = Some(n),
                None => return fail("--last-secs needs a number"),
            },
            "-h" | "--help" => {
                eprintln!(
                    "usage: trace_summary <trace-*.json> [--top N] [--last-secs N]\n\
                     \n\
                     Ranks spans in a Bevy chrome trace by self time. Produce one with\n\
                     `make profile windows` (see docs/perf-remote.md).\n\
                     \n\
                     --last-secs restricts the report to the tail of the capture, which is\n\
                     what you want when the session also contains loading and moving around."
                );
                return ExitCode::SUCCESS;
            }
            other if path.is_none() => path = Some(other.to_string()),
            other => return fail(&format!("unexpected argument: {other}")),
        }
    }

    let Some(path) = path else {
        eprintln!("usage: trace_summary <trace-*.json> [--top N] [--last-secs N]");
        return ExitCode::FAILURE;
    };

    let file = match File::open(&path) {
        Ok(f) => f,
        Err(err) => return fail(&format!("cannot read {path}: {err}")),
    };
    let size = file.metadata().map(|m| m.len()).unwrap_or(0);
    eprintln!(
        "reading {:.2} GB from {path} ...",
        size as f64 / 1024.0 / 1024.0 / 1024.0
    );

    let mut reducer = Reducer::new(last_secs);
    // 8 MiB buffer: the file is routinely gigabytes and may live on a slow mount.
    let reader = BufReader::with_capacity(8 << 20, file);
    let mut malformed = 0u64;
    let mut lines = 0u64;
    for line in reader.lines() {
        let Ok(line) = line else { break };
        // The array wrapper and the separating commas are not part of any event.
        let trimmed = line
            .trim()
            .trim_start_matches('[')
            .trim_end_matches(',')
            .trim_end_matches(']');
        if trimmed.is_empty() {
            continue;
        }
        lines += 1;
        match serde_json::from_str::<RawEvent>(trimmed) {
            Ok(event) => reducer.push(event),
            // A truncated tail is the documented failure mode of killing the
            // game instead of closing it; keep the complete prefix.
            Err(_) => malformed += 1,
        }
        if lines % 20_000_000 == 0 {
            eprintln!("  {lines} events ...");
        }
    }

    if reducer.all.is_empty() {
        eprintln!(
            "No B/E/X spans found in {path}.\n\
             The usual cause is a RUST_LOG filter: system spans are INFO on `bevy_ecs::*`\n\
             targets, so a filter like `warn,client=info` keeps the log lines and drops\n\
             every span. Re-run with RUST_LOG unset."
        );
        return ExitCode::FAILURE;
    }

    let span_secs = (reducer.last_ts - reducer.first_ts.unwrap_or(0.0)) / 1_000_000.0;
    eprintln!(
        "{lines} events, {:.1} s captured{}{}\n",
        span_secs,
        if malformed > 0 {
            format!(", {malformed} unparsable (truncated tail?)")
        } else {
            String::new()
        },
        if reducer.unmatched > 0 {
            format!(", {} unmatched span ends", reducer.unmatched)
        } else {
            String::new()
        }
    );

    if let Some(window) = last_secs {
        let recent = reducer.recent_aggregate();
        report(
            &recent,
            top,
            &format!("Last {window}s of the capture (steady state)"),
        );
        println!();
    }
    report(
        &reducer.all,
        top,
        &format!("Whole capture ({span_secs:.1} s)"),
    );
    ExitCode::SUCCESS
}

fn fail(message: &str) -> ExitCode {
    eprintln!("{message}");
    ExitCode::FAILURE
}

fn report(spans: &Aggregate, top: usize, title: &str) {
    let mut rows: Vec<(&String, &Span)> = spans.iter().collect();
    rows.sort_by(|a, b| b.1.self_us.total_cmp(&a.1.self_us));
    let grand_total: f64 = rows.iter().map(|(_, s)| s.self_us).sum();

    println!("== {title}");
    // `mean us` is per call, so a span that runs once per frame reads directly
    // as its per-frame cost -- the number to hold against the frame budget, and
    // the column to read when `self ms` is large only because the call count is.
    println!(
        "{:<52} {:>9} {:>9} {:>8} {:>10} {:>7}",
        "span", "self ms", "total ms", "calls", "mean us", "self %"
    );
    println!("{}", "-".repeat(101));
    for (name, span) in rows.iter().take(top) {
        println!(
            "{:<52} {:>9.1} {:>9.1} {:>8} {:>10.1} {:>6.1}%",
            truncate(name, 52),
            span.self_us / 1000.0,
            span.total_us / 1000.0,
            span.calls,
            span.self_us / span.calls.max(1) as f64,
            pct(span.self_us, grand_total),
        );
    }
    println!("{}", "-".repeat(101));
    println!(
        "{:<52} {:>9.1}   ({} distinct spans)",
        "total self time",
        grand_total / 1000.0,
        rows.len()
    );
}

fn pct(part: f64, whole: f64) -> f64 {
    if whole <= f64::EPSILON {
        0.0
    } else {
        part / whole * 100.0
    }
}

/// Bevy's system spans are full Rust paths and routinely exceed the column;
/// the tail is the identifying half, so trim from the front.
fn truncate(name: &str, width: usize) -> String {
    if name.chars().count() <= width {
        return name.to_string();
    }
    let tail: String = name
        .chars()
        .skip(name.chars().count() - (width - 1))
        .collect();
    format!("~{tail}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(lines: &[&str], window: Option<i64>) -> Reducer {
        let mut r = Reducer::new(window);
        for line in lines {
            r.push(serde_json::from_str::<RawEvent>(line).expect("valid event"));
        }
        r
    }

    /// The reduction that makes the report useful: a parent's time must not
    /// include its children's, or the root schedule wins every ranking.
    #[test]
    fn self_time_excludes_nested_spans() {
        let r = feed(
            &[
                r#"{"ph":"B","ts":0.0,"pid":1,"tid":1,"name":"schedule"}"#,
                r#"{"ph":"B","ts":10.0,"pid":1,"tid":1,"name":"system"}"#,
                r#"{"ph":"E","ts":60.0,"pid":1,"tid":1}"#,
                r#"{"ph":"E","ts":100.0,"pid":1,"tid":1}"#,
            ],
            None,
        );
        assert_eq!(r.all["system"].self_us, 50.0);
        assert_eq!(r.all["schedule"].total_us, 100.0);
        assert_eq!(r.all["schedule"].self_us, 50.0);
    }

    /// Threads are interleaved in the file; a shared stack would pop the wrong
    /// span and attribute one thread's time to another's parent.
    #[test]
    fn threads_keep_separate_stacks() {
        let r = feed(
            &[
                r#"{"ph":"B","ts":0.0,"pid":1,"tid":1,"name":"a"}"#,
                r#"{"ph":"B","ts":1.0,"pid":1,"tid":2,"name":"b"}"#,
                r#"{"ph":"E","ts":5.0,"pid":1,"tid":1}"#,
                r#"{"ph":"E","ts":9.0,"pid":1,"tid":2}"#,
            ],
            None,
        );
        assert_eq!(r.all["a"].self_us, 5.0);
        assert_eq!(r.all["b"].self_us, 8.0);
    }

    /// `X` events carry their own duration and must be charged to their parent.
    #[test]
    fn complete_events_are_charged_to_their_parent() {
        let r = feed(
            &[
                r#"{"ph":"B","ts":0.0,"pid":1,"tid":1,"name":"parent"}"#,
                r#"{"ph":"X","ts":1.0,"dur":30.0,"pid":1,"tid":1,"name":"leaf"}"#,
                r#"{"ph":"E","ts":100.0,"pid":1,"tid":1}"#,
            ],
            None,
        );
        assert_eq!(r.all["leaf"].self_us, 30.0);
        assert_eq!(r.all["parent"].self_us, 70.0);
    }

    /// An `E` with no `B` is a span that opened before the capture. It must be
    /// counted and skipped, never popped off another span's stack.
    #[test]
    fn unmatched_end_is_counted_not_misattributed() {
        let r = feed(
            &[
                r#"{"ph":"E","ts":5.0,"pid":1,"tid":1}"#,
                r#"{"ph":"B","ts":6.0,"pid":1,"tid":1,"name":"a"}"#,
                r#"{"ph":"E","ts":9.0,"pid":1,"tid":1}"#,
            ],
            None,
        );
        assert_eq!(r.unmatched, 1);
        assert_eq!(r.all["a"].self_us, 3.0);
    }

    /// The whole point of --last-secs: a long capture mixes loading with steady
    /// state, and only the tail describes the frame being asked about.
    #[test]
    fn last_secs_window_drops_older_buckets() {
        // "loading" closes at t=1s, "steady" at t=10s; a 3s window keeps only steady.
        let r = feed(
            &[
                r#"{"ph":"B","ts":0.0,"pid":1,"tid":1,"name":"loading"}"#,
                r#"{"ph":"E","ts":1000000.0,"pid":1,"tid":1}"#,
                r#"{"ph":"B","ts":9000000.0,"pid":1,"tid":1,"name":"steady"}"#,
                r#"{"ph":"E","ts":10000000.0,"pid":1,"tid":1}"#,
            ],
            Some(3),
        );
        let recent = r.recent_aggregate();
        assert!(recent.contains_key("steady"));
        assert!(!recent.contains_key("loading"));
        assert!(r.all.contains_key("loading"), "whole-capture keeps both");
    }

    /// Names carrying JSON escapes cannot be borrowed from the line buffer;
    /// Cow has to fall back to an owned string rather than failing the parse.
    #[test]
    fn escaped_names_still_parse() {
        let r = feed(
            &[
                r#"{"ph":"B","ts":0.0,"pid":1,"tid":1,"name":"system: name=\"foo::bar\""}"#,
                r#"{"ph":"E","ts":7.0,"pid":1,"tid":1}"#,
            ],
            None,
        );
        assert_eq!(r.all[r#"system: name="foo::bar""#].self_us, 7.0);
    }
}
