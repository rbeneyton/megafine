use std::borrow::Cow;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum TimeUnit {
    Microsecond,
    Millisecond,
    Second,
}

impl TimeUnit {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "us" | "µs" | "microsecond" => Some(TimeUnit::Microsecond),
            "ms" | "millisecond" => Some(TimeUnit::Millisecond),
            "s" | "second" => Some(TimeUnit::Second),
            _ => None,
        }
    }

    fn factor(self) -> f64 {
        match self {
            TimeUnit::Microsecond => 1e6,
            TimeUnit::Millisecond => 1e3,
            TimeUnit::Second => 1.0,
        }
    }

    fn suffix(self) -> &'static str {
        match self {
            TimeUnit::Microsecond => "µs",
            TimeUnit::Millisecond => "ms",
            TimeUnit::Second => "s",
        }
    }
}

/// How a metric's values are formatted: durations, byte sizes or plain counts.
#[derive(Clone, Copy, PartialEq)]
pub enum MetricKind {
    Time,
    Bytes,
    Count,
}

/// Format one value of a metric. `unit` (when set) forces the time unit and is
/// ignored by the other kinds.
pub fn format_metric(v: f64, kind: MetricKind, unit: Option<TimeUnit>, precision: usize) -> String {
    match kind {
        MetricKind::Time => format_time(v, unit.unwrap_or_else(|| auto_unit(v)), precision),
        MetricKind::Bytes => format_bytes(v as u64),
        MetricKind::Count => format_count(v),
    }
}

/// Pick a human-friendly unit for a duration given in seconds.
pub fn auto_unit(seconds: f64) -> TimeUnit {
    if seconds < 1e-3 {
        TimeUnit::Microsecond
    } else if seconds < 1.0 {
        TimeUnit::Millisecond
    } else {
        TimeUnit::Second
    }
}

/// Just the numeric part of a duration in `unit` (no suffix), for column layout.
fn time_value(seconds: f64, unit: TimeUnit, precision: usize) -> String {
    format!("{:.precision$}", seconds * unit.factor())
}

pub fn format_time(seconds: f64, unit: TimeUnit, precision: usize) -> String {
    format!("{} {}", time_value(seconds, unit, precision), unit.suffix())
}

const BYTE_UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];

/// Index into `BYTE_UNITS` of the largest binary unit not exceeding `bytes`.
fn byte_unit(bytes: u64) -> usize {
    let mut unit = 0;
    let mut limit = 1u64 << 10;
    while bytes >= limit && unit < BYTE_UNITS.len() - 1 {
        limit <<= 10;
        unit += 1;
    }
    unit
}

/// Just the numeric part of a byte count in `BYTE_UNITS[unit]` (no suffix).
fn format_byte_value(bytes: u64, unit: usize) -> String {
    if unit == 0 {
        format!("{bytes}")
    } else {
        format!("{:.1}", bytes as f64 / (1u64 << (10 * unit)) as f64)
    }
}

/// Human-friendly memory size using binary (1024) units.
pub fn format_bytes(bytes: u64) -> String {
    let unit = byte_unit(bytes);
    format!("{} {}", format_byte_value(bytes, unit), BYTE_UNITS[unit])
}

/// Human-friendly duration for the live ETA: coarse, integer fields.
pub fn format_duration(seconds: f64) -> String {
    let s = seconds.round() as u64;
    let (h, m, s) = (s / 3600, (s % 3600) / 60, s % 60);
    match (h, m) {
        (0, 0) => format!("{s} s"),
        (0, _) => format!("{m}m {s:02}s"),
        _ => format!("{h}h {m:02}m {s:02}s"),
    }
}

/// Truncate `s` to at most `max` display columns, keeping the start and the end
/// with a '…' in the middle (commands often differ at both ends, not just the
/// start). The front keeps the extra column when `max` is even. Borrows `s`
/// unchanged (no allocation) when it already fits.
pub fn truncate(s: &str, max: usize) -> Cow<'_, str> {
    if s.chars().count() <= max {
        return Cow::Borrowed(s);
    }
    let chars: Vec<char> = s.chars().collect();
    if max <= 1 {
        return Cow::Owned(chars[..max].iter().collect());
    }
    let budget = max - 1; // one column for the '…'
    let front = budget.div_ceil(2);
    let back = budget - front;
    let mut out: String = chars[..front].iter().collect();
    out.push('…');
    out.extend(&chars[chars.len() - back..]);
    Cow::Owned(out)
}

/// Format a large count with an SI suffix: `1234567` → `1.235 M`; values
/// under 10 000 stay plain integers.
pub fn format_count(v: f64) -> String {
    const UNITS: [&str; 4] = ["k", "M", "G", "T"];
    if v < 10_000.0 {
        return format!("{v:.0}");
    }
    let mut v = v / 1000.0;
    let mut unit = 0;
    while v >= 1000.0 && unit < UNITS.len() - 1 {
        v /= 1000.0;
        unit += 1;
    }
    format!("{v:.3} {}", UNITS[unit])
}

/// A row's live relation to the reference command, mirroring the final ranking.
#[derive(Clone, Copy)]
pub enum Relative {
    /// This row is the reference command itself.
    Reference,
    /// `center / reference_center`, with the propagated uncertainty on it when
    /// both stddevs are known.
    Ratio { ratio: f64, stddev: Option<f64> },
}

/// The `+x.xx% (± u.uu)` relative-speed cell, shared by the live counters and
/// the final ranking. Right-aligned to `pct_w`/`unc_w` so the decimal points
/// line up down a column; the uncertainty appears only when this row has one
/// and the column exists.
pub fn relative_cell(
    ratio: f64,
    stddev: Option<f64>,
    pct_w: usize,
    unc_w: Option<usize>,
) -> String {
    let pct = format!("{:+.2}", (ratio - 1.0) * 100.0);
    let mut cell = format!("{pct:>pct_w$}%");
    if let (Some(stddev), Some(uw)) = (stddev, unc_w) {
        cell.push_str(&format!(" (± {:>uw$})", format!("{:.2}", stddev * 100.0)));
    }
    cell
}

/// A row's live perf-counter column values: means over the runs so far.
#[derive(Clone, Copy)]
pub struct PerfCell {
    pub instr: f64,
    pub ipc: f64,
    pub cache_misses: f64,
    pub branch_misses: f64,
}

/// One command's live progress, in raw values; rendered together with its peers.
pub struct CounterRow<'a> {
    pub label: &'a str,
    pub count: u64,
    /// The estimator's central value (mean or percentile) of the times so far.
    pub center: f64,
    pub std: Option<f64>,
    pub peak_rss: u64,
    /// The perf-counter columns, when --counters is on.
    pub perf: Option<PerfCell>,
    /// Standing against the reference command, when it can already be shown.
    pub relative: Option<Relative>,
}

/// Render all live counter lines as a column-aligned block. The center/σ
/// columns show the selected metric (`kind`): time shares one unit —
/// `forced_unit` (if set) or the lowest unit across the rows — and the peak
/// RSS column shares the lowest unit across the rows.
pub fn render_counters(
    rows: &[CounterRow],
    kind: MetricKind,
    forced_unit: Option<TimeUnit>,
    precision: usize,
    budget: usize,
) -> Vec<String> {
    let present: Vec<&CounterRow> = rows.iter().filter(|r| r.count > 0).collect();
    if present.is_empty() {
        return rows
            .iter()
            .map(|r| {
                let label = truncate(r.label, budget.saturating_sub(": pending".chars().count()));
                format!("{label}: pending")
            })
            .collect();
    }

    // Center/σ cells are pre-rendered: their unit depends on the metric kind,
    // and right-aligning the finished cells keeps the columns lined up (time
    // rows share one unit, so their decimal points align too).
    let metric_cell: Box<dyn Fn(f64) -> String> = match kind {
        MetricKind::Time => {
            let unit = forced_unit
                .unwrap_or_else(|| present.iter().map(|r| auto_unit(r.center)).min().unwrap());
            Box::new(move |v| format_time(v, unit, precision))
        }
        MetricKind::Bytes => Box::new(|v| format_bytes(v as u64)),
        MetricKind::Count => Box::new(format_count),
    };
    let center_cells: Vec<String> = rows
        .iter()
        .map(|r| {
            if r.count == 0 {
                String::new()
            } else {
                metric_cell(r.center)
            }
        })
        .collect();
    let std_cells: Vec<Option<String>> = rows
        .iter()
        .map(|r| (r.count > 0).then(|| r.std.map(&metric_cell)).flatten())
        .collect();

    let rss_unit = present
        .iter()
        .filter(|r| r.peak_rss > 0)
        .map(|r| byte_unit(r.peak_rss))
        .min();

    let label_w = rows
        .iter()
        .map(|r| r.label.chars().count())
        .max()
        .unwrap_or(0);
    let count_w = present
        .iter()
        .map(|r| r.count.to_string().len())
        .max()
        .unwrap();
    let center_w = center_cells
        .iter()
        .map(|c| c.chars().count())
        .max()
        .unwrap();
    let std_w = std_cells.iter().flatten().map(|c| c.chars().count()).max();
    let rss_w = rss_unit.map(|u| {
        present
            .iter()
            .filter(|r| r.peak_rss > 0)
            .map(|r| format_byte_value(r.peak_rss, u).len())
            .max()
            .unwrap()
    });
    let perf_w = |cell: fn(&PerfCell) -> String| {
        present
            .iter()
            .filter_map(|r| r.perf.as_ref().map(|p| cell(p).len()))
            .max()
    };
    let instr_w = perf_w(|p| format_count(p.instr));
    let ipc_w = perf_w(|p| format!("{:.2}", p.ipc));
    let cache_w = perf_w(|p| format_count(p.cache_misses));
    let branch_w = perf_w(|p| format_count(p.branch_misses));

    // The relative column mirrors the final ranking's tail: `reference` on the
    // reference row, `+x.xx% (± u.uu)` on the others, decimal-aligned.
    let pct_w = present
        .iter()
        .filter_map(|r| match r.relative {
            Some(Relative::Ratio { ratio, .. }) => {
                Some(format!("{:+.2}", (ratio - 1.0) * 100.0).len())
            }
            _ => None,
        })
        .max();
    let unc_w = present
        .iter()
        .filter_map(|r| match r.relative {
            Some(Relative::Ratio {
                stddev: Some(stddev),
                ..
            }) => Some(format!("{:.2}", stddev * 100.0).len()),
            _ => None,
        })
        .max();
    let rel_cells: Vec<String> = rows
        .iter()
        .map(|r| match (r.relative, pct_w) {
            _ if r.count == 0 => String::new(),
            (Some(Relative::Reference), _) => "reference".to_string(),
            (Some(Relative::Ratio { ratio, stddev }), Some(pw)) => {
                relative_cell(ratio, stddev, pw, unc_w)
            }
            _ => String::new(),
        })
        .collect();

    // Cap the label column so the metric columns that follow it stay visible
    // within `budget`. The tail width is fixed once the columns above are known.
    let fixed_tail = 2 + count_w + 1 + 4 + 2 + center_w;
    let std_tail = std_w.map_or(0, |sw| 3 + sw);
    let peak_tail = match (rss_unit, rss_w) {
        (Some(u), Some(rw)) => 7 + rw + 1 + BYTE_UNITS[u].chars().count(),
        _ => 0,
    };
    let perf_tail = match (instr_w, ipc_w, cache_w, branch_w) {
        // "  {i} instr  IPC {p}  {c} cache-miss  {b} branch-miss"
        (Some(iw), Some(pw), Some(cw), Some(bw)) => {
            (2 + iw + 6) + (2 + 4 + pw) + (2 + cw + 11) + (2 + bw + 12)
        }
        _ => 0,
    };
    let rel_w = rel_cells
        .iter()
        .map(|c| c.chars().count())
        .max()
        .unwrap_or(0);
    let rel_tail = if rel_w > 0 { 2 + rel_w } else { 0 };
    let label_w = label_w
        .min(budget.saturating_sub(fixed_tail + std_tail + peak_tail + perf_tail + rel_tail));

    rows.iter()
        .zip(&rel_cells)
        .zip(center_cells.iter().zip(&std_cells))
        .map(|((x, rel), (center, std_cell))| {
            let label = truncate(x.label, label_w);
            if x.count == 0 {
                return format!("{label:<label_w$}  pending");
            }
            let runs = if x.count == 1 { "run" } else { "runs" };
            let mut line = format!(
                "{label:<label_w$}  {:>count_w$} {runs:<4}  {center:>center_w$}",
                x.count,
            );
            // Reserve the `± σ` segment on every row so the peak column aligns
            // even while a freshly-started command still has a single run.
            if let Some(sw) = std_w {
                match std_cell {
                    Some(sc) => line.push_str(&format!(" ± {sc:>sw$}")),
                    None => line.push_str(&" ".repeat(3 + sw)),
                }
            }
            if let (Some(u), Some(rw)) = (rss_unit, rss_w)
                && x.peak_rss > 0
            {
                line.push_str(&format!(
                    "  peak {:>rw$} {}",
                    format_byte_value(x.peak_rss, u),
                    BYTE_UNITS[u],
                ));
            }
            if let (Some(p), Some(iw), Some(pw), Some(cw), Some(bw)) =
                (&x.perf, instr_w, ipc_w, cache_w, branch_w)
            {
                line.push_str(&format!(
                    "  {:>iw$} instr  IPC {:>pw$}  {:>cw$} cache-miss  {:>bw$} branch-miss",
                    format_count(p.instr),
                    format!("{:.2}", p.ipc),
                    format_count(p.cache_misses),
                    format_count(p.branch_misses),
                ));
            }
            if !rel.is_empty() {
                line.push_str(&format!("  {rel}"));
            }
            line
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_time_unit() {
        // TimeUnit has no Debug, so compare with `==` rather than assert_eq!.
        assert!(TimeUnit::parse("us") == Some(TimeUnit::Microsecond));
        assert!(TimeUnit::parse("µs") == Some(TimeUnit::Microsecond));
        assert!(TimeUnit::parse("ms") == Some(TimeUnit::Millisecond));
        assert!(TimeUnit::parse("second") == Some(TimeUnit::Second));
        assert!(TimeUnit::parse("ns").is_none());
    }

    #[test]
    fn auto_unit_boundaries() {
        assert!(auto_unit(1e-4) == TimeUnit::Microsecond);
        assert!(auto_unit(0.5) == TimeUnit::Millisecond);
        assert!(auto_unit(5.0) == TimeUnit::Second);
    }

    #[test]
    fn time_formatting() {
        assert_eq!(format_time(0.0015, TimeUnit::Millisecond, 3), "1.500 ms");
        assert_eq!(format_time(2.0, TimeUnit::Second, 3), "2.000 s");
        assert_eq!(format_time(0.0015, TimeUnit::Millisecond, 1), "1.5 ms");
        assert_eq!(format_time(2.0, TimeUnit::Second, 0), "2 s");
    }

    #[test]
    fn byte_formatting() {
        assert_eq!(format_bytes(500), "500 B");
        assert_eq!(format_bytes(1024), "1.0 KB");
        assert_eq!(format_bytes(5 << 20), "5.0 MB");
    }

    #[test]
    fn count_formatting() {
        assert_eq!(format_count(3.0), "3");
        assert_eq!(format_count(9_999.0), "9999");
        assert_eq!(format_count(12_345.0), "12.345 k");
        assert_eq!(format_count(1_234_567.0), "1.235 M");
        assert_eq!(format_count(2.5e12), "2.500 T");
    }

    #[test]
    fn duration_formatting() {
        assert_eq!(format_duration(0.4), "0 s");
        assert_eq!(format_duration(42.4), "42 s");
        assert_eq!(format_duration(154.2), "2m 34s");
        assert_eq!(format_duration(3723.0), "1h 02m 03s");
    }

    #[test]
    fn truncate_short_unchanged() {
        assert_eq!(truncate("abc", 5), "abc");
        assert_eq!(truncate("abc", 3), "abc");
    }

    #[test]
    fn truncate_keeps_front_and_back() {
        // max 4 → budget 3: front = ceil(3/2)=2 ('ab'), '…', back = 1 ('f').
        assert_eq!(truncate("abcdef", 4), "ab…f");
    }

    #[test]
    fn truncate_multibyte() {
        // Counts characters, not bytes; must not split a multibyte char.
        assert_eq!(truncate("ééééé", 3), "é…é");
    }

    #[test]
    fn truncate_tiny_budget() {
        assert_eq!(truncate("abcdef", 1), "a");
        assert_eq!(truncate("abcdef", 0), "");
    }

    /// A `CounterRow` without peak RSS, so the relative column follows the times.
    fn counter_row(
        count: u64,
        center: f64,
        std: Option<f64>,
        relative: Option<Relative>,
    ) -> CounterRow<'static> {
        CounterRow {
            label: "x",
            count,
            center,
            std,
            peak_rss: 0,
            perf: None,
            relative,
        }
    }

    #[test]
    fn counters_relative_column() {
        let rows = [
            counter_row(3, 1.0, Some(0.1), Some(Relative::Reference)),
            counter_row(
                3,
                1.1,
                Some(0.1),
                Some(Relative::Ratio {
                    ratio: 1.1044,
                    stddev: Some(0.0587),
                }),
            ),
        ];
        let lines = render_counters(&rows, MetricKind::Time, None, 3, 200);
        assert!(lines[0].ends_with("  reference"), "{}", lines[0]);
        assert!(lines[1].ends_with("  +10.44% (± 5.87)"), "{}", lines[1]);
    }

    #[test]
    fn counters_relative_alignment() {
        // Percentages of different widths right-align; a row without an
        // uncertainty (single run on either side) just ends at the '%'.
        let rows = [
            counter_row(3, 1.0, Some(0.1), Some(Relative::Reference)),
            counter_row(
                1,
                1.095,
                None,
                Some(Relative::Ratio {
                    ratio: 1.095,
                    stddev: None,
                }),
            ),
            counter_row(
                3,
                2.234,
                Some(0.1),
                Some(Relative::Ratio {
                    ratio: 2.234,
                    stddev: Some(0.123),
                }),
            ),
        ];
        let lines = render_counters(&rows, MetricKind::Time, None, 3, 200);
        assert!(lines[1].ends_with("    +9.50%"), "{}", lines[1]);
        assert!(lines[2].ends_with("  +123.40% (± 12.30)"), "{}", lines[2]);
    }

    #[test]
    fn counters_without_relative_have_no_column() {
        let rows = [counter_row(2, 0.5, Some(0.1), None)];
        let lines = render_counters(&rows, MetricKind::Time, None, 3, 200);
        assert!(lines[0].ends_with("ms"), "{}", lines[0]);
    }

    #[test]
    fn metric_formatting_per_kind() {
        assert_eq!(
            format_metric(0.0015, MetricKind::Time, Some(TimeUnit::Millisecond), 3),
            "1.500 ms"
        );
        assert_eq!(
            format_metric((5 << 20) as f64, MetricKind::Bytes, None, 3),
            "5.0 MB"
        );
        assert_eq!(
            format_metric(12_345.0, MetricKind::Count, None, 3),
            "12.345 k"
        );
    }

    #[test]
    fn counters_center_column_follows_kind() {
        // A byte-kind metric renders center/σ as sizes, not durations.
        let rows = [
            counter_row(3, (2 << 20) as f64, Some((1 << 10) as f64), None),
            counter_row(3, (4 << 20) as f64, Some((1 << 10) as f64), None),
        ];
        let lines = render_counters(&rows, MetricKind::Bytes, None, 3, 200);
        assert!(lines[0].contains("MB"), "{}", lines[0]);
        assert!(!lines[0].contains("ms"), "{}", lines[0]);
        // A count-kind metric renders plain SI counts.
        let rows = [counter_row(2, 12_345.0, None, None)];
        let lines = render_counters(&rows, MetricKind::Count, None, 3, 200);
        assert!(lines[0].contains("12.345 k"), "{}", lines[0]);
    }
}
