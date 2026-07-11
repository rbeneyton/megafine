use crate::format::{MetricCell, MetricKind};
use crate::perf::PerfCounts;
use crate::stats;
use crate::stats::Estimator;

/// The per-run quantity every statistic (--estimator, ranking, --target,
/// t-test verdict, live display) is computed on (--metric). All variants are
/// lower-is-better; derived quantities like IPC are excluded on purpose.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Metric {
    Time,
    User,
    Sys,
    Rss,
    MajorFaults,
    MinorFaults,
    VolCtx,
    InvolCtx,
    Instructions,
    Cycles,
    CacheMisses,
    BranchMisses,
}

impl Metric {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "time" => Metric::Time,
            "user" => Metric::User,
            "sys" => Metric::Sys,
            "rss" => Metric::Rss,
            "major-faults" => Metric::MajorFaults,
            "minor-faults" => Metric::MinorFaults,
            "vol-ctx" => Metric::VolCtx,
            "invol-ctx" => Metric::InvolCtx,
            "instructions" => Metric::Instructions,
            "cycles" => Metric::Cycles,
            "cache-misses" => Metric::CacheMisses,
            "branch-misses" => Metric::BranchMisses,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Metric::Time => "time",
            Metric::User => "user",
            Metric::Sys => "sys",
            Metric::Rss => "rss",
            Metric::MajorFaults => "major-faults",
            Metric::MinorFaults => "minor-faults",
            Metric::VolCtx => "vol-ctx",
            Metric::InvolCtx => "invol-ctx",
            Metric::Instructions => "instructions",
            Metric::Cycles => "cycles",
            Metric::CacheMisses => "cache-misses",
            Metric::BranchMisses => "branch-misses",
        }
    }

    /// The metric's value in one run. Counter metrics imply --counters, so
    /// the 0.0 fallback on a missing `counters` is defensive only.
    pub fn value(self, e: &Execution) -> f64 {
        let counter =
            |get: fn(&PerfCounts) -> u64| e.counters.as_ref().map_or(0.0, |c| get(c) as f64);
        match self {
            Metric::Time => e.wall_clock,
            Metric::User => e.time_user,
            Metric::Sys => e.time_system,
            Metric::Rss => e.max_rss as f64,
            Metric::MajorFaults => e.major_faults as f64,
            Metric::MinorFaults => e.minor_faults as f64,
            Metric::VolCtx => e.vol_ctx_switches as f64,
            Metric::InvolCtx => e.invol_ctx_switches as f64,
            Metric::Instructions => counter(|c| c.instructions),
            Metric::Cycles => counter(|c| c.cycles),
            Metric::CacheMisses => counter(|c| c.cache_misses),
            Metric::BranchMisses => counter(|c| c.branch_misses),
        }
    }

    pub fn needs_counters(self) -> bool {
        matches!(
            self,
            Metric::Instructions | Metric::Cycles | Metric::CacheMisses | Metric::BranchMisses
        )
    }

    /// How the metric's values are formatted.
    pub fn kind(self) -> MetricKind {
        match self {
            Metric::Time | Metric::User | Metric::Sys => MetricKind::Time,
            Metric::Rss => MetricKind::Bytes,
            _ => MetricKind::Count,
        }
    }

    /// The live-display cell that carries this metric (flagged in bold when it
    /// is not the wall clock). Cycles has no perf cell (IPC is shown instead),
    /// so it falls back to a section of its own like user, sys, faults and ctx.
    pub fn live_cell(self) -> MetricCell {
        match self {
            Metric::Time => MetricCell::Time,
            Metric::Rss => MetricCell::Peak,
            Metric::Instructions => MetricCell::Instr,
            Metric::CacheMisses => MetricCell::CacheMisses,
            Metric::BranchMisses => MetricCell::BranchMisses,
            _ => MetricCell::Own(self.kind(), self.name()),
        }
    }
}

/// The measured outcome of one command execution: its wall-clock time and the
/// resource usage reported by `wait4`. A non-zero exit is surfaced as an error
/// instead, unless --ignore-failure keeps it as a `failed` measurement.
#[derive(Default)]
pub struct Execution {
    /// Elapsed wall clock time, in seconds.
    pub wall_clock: f64,
    /// Time spent in user mode, in seconds.
    pub time_user: f64,
    /// Time spent in kernel mode, in seconds.
    pub time_system: f64,
    /// Peak resident set size, in bytes.
    pub max_rss: u64,
    /// Major (I/O) and minor page faults.
    pub major_faults: u64,
    pub minor_faults: u64,
    /// Voluntary and involuntary context switches.
    pub vol_ctx_switches: u64,
    pub invol_ctx_switches: u64,
    /// Hardware counters, when --counters is on.
    pub counters: Option<PerfCounts>,
    /// The command exited non-zero (kept only under --ignore-failure).
    pub failed: bool,
}

/// All measurements collected for one benchmarked command.
pub struct BenchmarkResult {
    pub label: String,
    pub measurements: Vec<Execution>,
}

impl BenchmarkResult {
    pub fn times(&self, field: impl Fn(&Execution) -> f64) -> Vec<f64> {
        self.measurements.iter().map(field).collect()
    }
}

pub struct NormBenchmark<'a> {
    pub result: &'a BenchmarkResult,
    /// The estimator's central value (mean or percentile) of the metric.
    pub center: f64,
    /// Sample stddev of the metric's values (`None` for fewer than two runs).
    pub spread: Option<f64>,
    /// `center / reference_center` (the first command's center).
    pub ratio: f64,
    /// Propagated uncertainty on `ratio`, if both stddevs are known.
    pub stddev: Option<f64>,
    pub is_reference: bool,
}

/// Compare every result against the `reference`-th one. Returns `None` if the
/// reference central value is zero (a relative ratio would be meaningless).
/// The caller guarantees `reference < results.len()`.
pub fn compute(
    results: &[BenchmarkResult],
    reference: usize,
    estimator: Estimator,
    metric: Metric,
) -> Option<Vec<NormBenchmark<'_>>> {
    // Each command's metric (center, stddev), computed once.
    let summaries: Vec<(f64, Option<f64>)> = results
        .iter()
        .map(|r| {
            let mut times = r.times(|x| metric.value(x));
            times.sort_unstable_by(f64::total_cmp);
            (estimator.value(&times), stats::mean_stddev(&times).1)
        })
        .collect();

    let (ref_center, ref_stddev) = *summaries.get(reference)?;
    if ref_center == 0.0 {
        return None;
    }

    let relative = results
        .iter()
        .zip(&summaries)
        .enumerate()
        .map(|(idx, (result, &(center, stddev)))| {
            let ratio = center / ref_center;
            let ratio_stddev = stddev.zip(ref_stddev).map(|(stddev, ref_stddev)| {
                stats::ratio_stddev(center, stddev, ref_center, ref_stddev)
            });
            NormBenchmark {
                result,
                center,
                spread: stddev,
                ratio,
                stddev: ratio_stddev,
                is_reference: idx == reference,
            }
        })
        .collect();

    Some(relative)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `BenchmarkResult` with the given wall-clock samples (other fields unused
    /// by `compute`).
    fn result(label: &str, wall_clocks: &[f64]) -> BenchmarkResult {
        BenchmarkResult {
            label: label.to_string(),
            measurements: wall_clocks
                .iter()
                .map(|&wall_clock| Execution {
                    wall_clock,
                    ..Default::default()
                })
                .collect(),
        }
    }

    #[test]
    fn ratios_against_first() {
        let results = [result("a", &[0.1, 0.1]), result("b", &[0.2, 0.2])];
        let rel = compute(&results, 0, Estimator::Mean, Metric::Time).unwrap();
        assert!(rel[0].is_reference);
        assert!(!rel[1].is_reference);
        assert!((rel[0].ratio - 1.0).abs() < 1e-12);
        assert!((rel[1].ratio - 2.0).abs() < 1e-12);
    }

    #[test]
    fn ratios_against_chosen_reference() {
        let results = [result("a", &[0.1, 0.1]), result("b", &[0.2, 0.2])];
        let rel = compute(&results, 1, Estimator::Mean, Metric::Time).unwrap();
        assert!(!rel[0].is_reference);
        assert!(rel[1].is_reference);
        assert!((rel[0].ratio - 0.5).abs() < 1e-12);
        assert!((rel[1].ratio - 1.0).abs() < 1e-12);
    }

    #[test]
    fn reference_out_of_range_is_none() {
        let results = [result("a", &[0.1]), result("b", &[0.2])];
        assert!(compute(&results, 2, Estimator::Mean, Metric::Time).is_none());
    }

    #[test]
    fn zero_reference_mean_is_none() {
        let results = [result("a", &[0.0, 0.0]), result("b", &[0.2, 0.2])];
        assert!(compute(&results, 0, Estimator::Mean, Metric::Time).is_none());
    }

    #[test]
    fn percentile_estimator_changes_ratio() {
        // b's median is 1.0 (ratio 1) but its mean is 4.0 (ratio 4).
        let results = [
            result("a", &[1.0, 1.0, 1.0]),
            result("b", &[1.0, 1.0, 10.0]),
        ];
        let rel = compute(&results, 0, Estimator::Percentile(50.0), Metric::Time).unwrap();
        assert!((rel[1].ratio - 1.0).abs() < 1e-12);
        let rel = compute(&results, 0, Estimator::Mean, Metric::Time).unwrap();
        assert!((rel[1].ratio - 4.0).abs() < 1e-12);
    }

    #[test]
    fn metric_parse_all_names() {
        for name in [
            "time",
            "user",
            "sys",
            "rss",
            "major-faults",
            "minor-faults",
            "vol-ctx",
            "invol-ctx",
            "instructions",
            "cycles",
            "cache-misses",
            "branch-misses",
        ] {
            let m = Metric::parse(name).expect(name);
            assert_eq!(m.name(), name);
        }
        assert!(Metric::parse("ipc").is_none());
        assert!(Metric::parse("foo").is_none());
    }

    #[test]
    fn metric_value_extraction() {
        let e = Execution {
            wall_clock: 1.5,
            max_rss: 2048,
            ..Default::default()
        };
        assert_eq!(Metric::Time.value(&e), 1.5);
        assert_eq!(Metric::Rss.value(&e), 2048.0);
        // Counters absent: defensive 0.0, not a panic.
        assert_eq!(Metric::Instructions.value(&e), 0.0);
        let e = Execution {
            counters: Some(PerfCounts {
                instructions: 100,
                cycles: 50,
                cache_misses: 3,
                branch_misses: 2,
            }),
            ..Default::default()
        };
        assert_eq!(Metric::Instructions.value(&e), 100.0);
        assert_eq!(Metric::Cycles.value(&e), 50.0);
    }

    #[test]
    fn compute_on_rss_metric() {
        let mk = |label: &str, rss: u64| BenchmarkResult {
            label: label.to_string(),
            measurements: vec![Execution {
                max_rss: rss,
                ..Default::default()
            }],
        };
        let results = [mk("a", 1000), mk("b", 3000)];
        let rel = compute(&results, 0, Estimator::Mean, Metric::Rss).unwrap();
        assert!((rel[1].ratio - 3.0).abs() < 1e-12);
    }

    #[test]
    fn single_command_has_unit_ratio_and_no_stddev() {
        let results = [result("a", &[0.1])];
        let rel = compute(&results, 0, Estimator::Mean, Metric::Time).unwrap();
        assert_eq!(rel.len(), 1);
        assert!(rel[0].is_reference);
        assert!((rel[0].ratio - 1.0).abs() < 1e-12);
        assert!(rel[0].stddev.is_none());
    }
}
