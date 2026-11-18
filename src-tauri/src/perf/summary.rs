//! A perf log turned into the table of `docs/perf.md`: per event the number of
//! samples and the p50, p95 and maximum, next to the Section 4 budget.
//!
//! Percentiles are "nearest rank" (the smallest value with at least p percent of
//! the samples at or below it), so they are always values that were measured.
//! With only a handful of samples the p95 is simply the maximum, which the count
//! column makes visible; do not read a p95 off fewer than 20 samples.

use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::Path;

use super::{BoundedFile, Event, Sample};

/// The distribution of one event, durations in microseconds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stat {
    pub count: usize,
    pub p50: u64,
    pub p95: u64,
    pub max: u64,
}

/// Nearest rank percentile of an ascending slice. `pct` in 0..=100; 0 for an
/// empty slice.
pub fn percentile(sorted: &[u64], pct: u32) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let pct = pct.min(100) as usize;
    let rank = (pct * sorted.len()).div_ceil(100).max(1);
    sorted[rank - 1]
}

pub fn stat_of(mut micros: Vec<u64>) -> Option<Stat> {
    if micros.is_empty() {
        return None;
    }
    micros.sort_unstable();
    Some(Stat {
        count: micros.len(),
        p50: percentile(&micros, 50),
        p95: percentile(&micros, 95),
        max: *micros.last()?,
    })
}

/// One row per event that has samples, in the order of [`Event::ALL`].
pub fn summarize(samples: &[Sample]) -> Vec<(Event, Stat)> {
    Event::ALL
        .into_iter()
        .filter_map(|e| {
            let v: Vec<u64> = samples
                .iter()
                .filter(|s| s.event == e)
                .map(|s| s.micros)
                .collect();
            stat_of(v).map(|s| (e, s))
        })
        .collect()
}

pub fn parse_log(text: &str) -> Vec<Sample> {
    text.lines().filter_map(Sample::parse_line).collect()
}

fn ms(micros: u64) -> String {
    format!("{:.2}", micros as f64 / 1000.0)
}

/// The markdown table. The last column compares the p95 with the budget of the
/// event: "under" or "OVER", or "-" for events without one. It judges only the
/// machine and build the log came from.
pub fn render_markdown(rows: &[(Event, Stat)]) -> String {
    let mut out = String::from(
        "| Event | Samples | p50 ms | p95 ms | Max ms | Budget ms | p95 vs budget |\n\
         |---|---:|---:|---:|---:|---:|---|\n",
    );
    for (e, s) in rows {
        let (budget, verdict) = match e.budget_ms() {
            Some(b) => (
                format!("{b}"),
                if (s.p95 as f64 / 1000.0) <= b {
                    "under"
                } else {
                    "OVER"
                },
            ),
            None => ("-".to_owned(), "-"),
        };
        // Numbers and the event's fixed name, bound to neutral names first.
        let (what, runs) = (e.name(), s.count);
        let (p50, p95, worst) = (ms(s.p50), ms(s.p95), ms(s.max));
        let _ = writeln!(
            out,
            "| {what} | {runs} | {p50} | {p95} | {worst} | {budget} | {verdict} |"
        );
    }
    out
}

/// Read a log and its rotated older half, oldest first.
pub fn read_log(path: &Path) -> io::Result<Vec<Sample>> {
    let mut samples = Vec::new();
    if let Ok(old) = fs::read_to_string(BoundedFile::rotated_path(path)) {
        samples.extend(parse_log(&old));
    }
    samples.extend(parse_log(&fs::read_to_string(path)?));
    Ok(samples)
}

/// The whole report for a log file: a heading, the table, and a note on the
/// sample count. Errors only when the log cannot be read.
pub fn summarize_log(path: &Path) -> io::Result<String> {
    let samples = read_log(path)?;
    let rows = summarize(&samples);
    let samples_seen = samples.len();
    let mut md = format!("## Perf summary\n\n{samples_seen} samples.\n\n");
    if rows.is_empty() {
        md.push_str(
            "No samples yet. Start the app with `--perf`, use it, quit, and summarize again.\n",
        );
    } else {
        md.push_str(&render_markdown(&rows));
        md.push_str("\np95 of fewer than 20 samples is just the largest one.\n");
    }
    Ok(md)
}

/// `clipdump --perf-summary [path]`: print the report; the exit code. Prints to
/// stdout (a GUI build on Windows has no console of its own, so redirect:
/// `clipdump.exe --perf-summary perf.log > summary.md`).
pub fn run_cli(path: Option<&Path>) -> i32 {
    let Some(path) = path else {
        eprintln!("usage: clipdump --perf-summary <path to perf.log>");
        return 2;
    };
    match summarize_log(path) {
        Ok(md) => {
            println!("{md}");
            0
        }
        Err(e) => {
            eprintln!("clipdump: cannot read the perf log: {e}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(event: Event, micros: u64) -> Sample {
        Sample {
            at_ms: 1,
            event,
            micros,
            n: 0,
        }
    }

    #[test]
    fn nearest_rank_percentiles() {
        let v: Vec<u64> = (1..=100).collect();
        assert_eq!(percentile(&v, 50), 50);
        assert_eq!(percentile(&v, 95), 95);
        assert_eq!(percentile(&v, 100), 100);
        assert_eq!(percentile(&v, 0), 1, "never below the smallest");
        assert_eq!(percentile(&[7], 50), 7);
        assert_eq!(percentile(&[7], 95), 7);
        assert_eq!(percentile(&[], 95), 0);
        // Ten samples: p50 is the 5th, p95 the 10th.
        let ten: Vec<u64> = (1..=10).collect();
        assert_eq!((percentile(&ten, 50), percentile(&ten, 95)), (5, 10));
        // Twenty: p95 is the 19th, so one outlier does not decide it.
        let mut twenty: Vec<u64> = (1..=19).collect();
        twenty.push(1000);
        assert_eq!(percentile(&twenty, 95), 19);
    }

    #[test]
    fn stats_sort_their_input_and_skip_empty_events() {
        let st = stat_of(vec![30, 10, 20]).unwrap();
        assert_eq!(
            st,
            Stat {
                count: 3,
                p50: 20,
                p95: 30,
                max: 30
            }
        );
        assert_eq!(stat_of(vec![]), None);
    }

    #[test]
    fn rows_follow_the_event_order_and_only_cover_events_with_samples() {
        let samples = [
            s(Event::Ocr, 500_000),
            s(Event::SearchCore, 400),
            s(Event::SearchCore, 600),
            s(Event::SearchCore, 2000),
        ];
        let rows = summarize(&samples);
        assert_eq!(
            rows.iter().map(|r| r.0).collect::<Vec<_>>(),
            [Event::SearchCore, Event::Ocr]
        );
        assert_eq!(rows[0].1.count, 3);
        assert_eq!(rows[0].1.p50, 600);
        assert_eq!(rows[0].1.max, 2000);
    }

    #[test]
    fn the_table_compares_p95_with_the_budget() {
        let samples = [
            s(Event::SearchCore, 800),
            s(Event::SearchCore, 1200),
            s(Event::Retention, 5),
        ];
        let md = render_markdown(&summarize(&samples));
        assert!(
            md.contains("| search_core | 2 | 0.80 | 1.20 | 1.20 | 1 | OVER |"),
            "{md}"
        );
        assert!(
            md.contains("| retention | 1 | 0.01 | 0.01 | 0.01 | - | - |"),
            "{md}"
        );
        let fast = render_markdown(&summarize(&[s(Event::CopyText, 3000)]));
        assert!(
            fast.contains("| copy_text | 1 | 3.00 | 3.00 | 3.00 | 20 | under |"),
            "{fast}"
        );
    }

    #[test]
    fn a_log_with_noise_is_summarized_and_the_rotated_half_comes_first() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("perf.log");
        fs::write(
            BoundedFile::rotated_path(&path),
            format!("{}\n", s(Event::Ocr, 100).format_line()),
        )
        .unwrap();
        fs::write(
            &path,
            format!(
                "clipdump: loaded 5 items in 3 ms\nclipdump: {}\ngarbage\n{}\n",
                s(Event::Ocr, 300).format_line(),
                s(Event::Ocr, 200).format_line()
            ),
        )
        .unwrap();
        let samples = read_log(&path).unwrap();
        assert_eq!(
            samples.iter().map(|x| x.micros).collect::<Vec<_>>(),
            [100, 300, 200]
        );
        let md = summarize_log(&path).unwrap();
        assert!(md.contains("3 samples."), "{md}");
        assert!(
            md.contains("| ocr | 3 | 0.20 | 0.30 | 0.30 | 1000 | under |"),
            "{md}"
        );
    }

    #[test]
    fn an_empty_or_missing_log_is_said_plainly() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("perf.log");
        assert!(summarize_log(&path).is_err());
        fs::write(&path, "").unwrap();
        assert!(summarize_log(&path).unwrap().contains("No samples yet"));
        assert_eq!(run_cli(None), 2);
        assert_eq!(run_cli(Some(&dir.path().join("nope.log"))), 1);
    }
}
