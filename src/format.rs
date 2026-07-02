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
fn time_value(seconds: f64, unit: TimeUnit) -> String {
    format!("{:.3}", seconds * unit.factor())
}

pub fn format_time(seconds: f64, unit: TimeUnit) -> String {
    format!("{} {}", time_value(seconds, unit), unit.suffix())
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

/// Truncate `s` to at most `max` display columns, keeping the start and the end
/// with a '…' in the middle (commands often differ at both ends, not just the
/// start). The front keeps the extra column when `max` is even.
pub fn truncate(s: &str, max: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max {
        return s.to_string();
    }
    if max <= 1 {
        return chars[..max].iter().collect();
    }
    let budget = max - 1; // one column for the '…'
    let front = budget.div_ceil(2);
    let back = budget - front;
    let mut out: String = chars[..front].iter().collect();
    out.push('…');
    out.extend(&chars[chars.len() - back..]);
    out
}

/// A row's live relation to the reference command, mirroring the final ranking.
#[derive(Clone, Copy)]
pub enum Relative {
    /// This row is the reference command itself.
    Reference,
    /// `mean / reference_mean`, with the propagated uncertainty on it when
    /// both stddevs are known.
    Ratio { ratio: f64, stddev: Option<f64> },
}

/// One command's live progress, in raw values; rendered together with its peers.
pub struct CounterRow<'a> {
    pub label: &'a str,
    pub count: u64,
    pub mean: f64,
    pub std: Option<f64>,
    pub peak_rss: u64,
    /// Standing against the reference command, when it can already be shown.
    pub relative: Option<Relative>,
}

/// Render all live counter lines as a column-aligned block. Every column shares
/// one unit — `forced_unit` (if set) or the lowest unit across the rows for time,
/// and the lowest unit across the rows for peak RSS.
pub fn render_counters(
    rows: &[CounterRow],
    forced_unit: Option<TimeUnit>,
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

    let time_unit =
        forced_unit.unwrap_or_else(|| present.iter().map(|r| auto_unit(r.mean)).min().unwrap());
    let suffix = time_unit.suffix();
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
    let mean_w = present
        .iter()
        .map(|r| time_value(r.mean, time_unit).len())
        .max()
        .unwrap();
    let std_w = present
        .iter()
        .filter_map(|r| r.std.map(|s| time_value(s, time_unit).len()))
        .max();
    let rss_w = rss_unit.map(|u| {
        present
            .iter()
            .filter(|r| r.peak_rss > 0)
            .map(|r| format_byte_value(r.peak_rss, u).len())
            .max()
            .unwrap()
    });

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
                let pct = format!("{:+.2}", (ratio - 1.0) * 100.0);
                let mut cell = format!("{pct:>pw$}%");
                if let (Some(stddev), Some(uw)) = (stddev, unc_w) {
                    cell.push_str(&format!(" (± {:>uw$})", format!("{:.2}", stddev * 100.0)));
                }
                cell
            }
            _ => String::new(),
        })
        .collect();

    // Cap the label column so the duration columns that follow it stay visible
    // within `budget`. The tail width is fixed once the columns above are known.
    let suffix_w = suffix.chars().count();
    let fixed_tail = 2 + count_w + 1 + 4 + 2 + mean_w + 1 + suffix_w;
    let std_tail = std_w.map_or(0, |sw| 3 + sw + 1 + suffix_w);
    let peak_tail = match (rss_unit, rss_w) {
        (Some(u), Some(rw)) => 7 + rw + 1 + BYTE_UNITS[u].chars().count(),
        _ => 0,
    };
    let rel_w = rel_cells
        .iter()
        .map(|c| c.chars().count())
        .max()
        .unwrap_or(0);
    let rel_tail = if rel_w > 0 { 2 + rel_w } else { 0 };
    let label_w = label_w.min(budget.saturating_sub(fixed_tail + std_tail + peak_tail + rel_tail));

    rows.iter()
        .zip(&rel_cells)
        .map(|(x, rel)| {
            let label = truncate(x.label, label_w);
            if x.count == 0 {
                return format!("{label:<label_w$}  pending");
            }
            let runs = if x.count == 1 { "run" } else { "runs" };
            let mut line = format!(
                "{label:<label_w$}  {:>count_w$} {runs:<4}  {:>mean_w$} {suffix}",
                x.count,
                time_value(x.mean, time_unit),
            );
            // Reserve the `± σ` segment on every row so the peak column aligns
            // even while a freshly-started command still has a single run.
            if let Some(sw) = std_w {
                match x.std {
                    Some(std) => {
                        line.push_str(&format!(" ± {:>sw$} {suffix}", time_value(std, time_unit)))
                    }
                    None => line.push_str(&" ".repeat(3 + sw + 1 + suffix.chars().count())),
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
        assert_eq!(format_time(0.0015, TimeUnit::Millisecond), "1.500 ms");
        assert_eq!(format_time(2.0, TimeUnit::Second), "2.000 s");
    }

    #[test]
    fn byte_formatting() {
        assert_eq!(format_bytes(500), "500 B");
        assert_eq!(format_bytes(1024), "1.0 KB");
        assert_eq!(format_bytes(5 << 20), "5.0 MB");
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
        mean: f64,
        std: Option<f64>,
        relative: Option<Relative>,
    ) -> CounterRow<'static> {
        CounterRow {
            label: "x",
            count,
            mean,
            std,
            peak_rss: 0,
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
        let lines = render_counters(&rows, None, 200);
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
        let lines = render_counters(&rows, None, 200);
        assert!(lines[1].ends_with("    +9.50%"), "{}", lines[1]);
        assert!(lines[2].ends_with("  +123.40% (± 12.30)"), "{}", lines[2]);
    }

    #[test]
    fn counters_without_relative_have_no_column() {
        let rows = [counter_row(2, 0.5, Some(0.1), None)];
        let lines = render_counters(&rows, None, 200);
        assert!(lines[0].ends_with("ms"), "{}", lines[0]);
    }
}
