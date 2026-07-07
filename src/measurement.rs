use crate::perf::PerfCounts;
use crate::stats;
use crate::stats::Estimator;

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
    /// The estimator's central value (mean or percentile) of the wall-clock times.
    pub center: f64,
    /// `center / reference_center` (the first command's center).
    pub ratio: f64,
    /// Propagated uncertainty on `ratio`, if both stddevs are known.
    pub stddev: Option<f64>,
    pub is_reference: bool,
}

/// Compare every result against the `reference`-th one. Returns `None` if the
/// reference central time is zero (relative speed would be meaningless). The
/// caller guarantees `reference < results.len()`.
pub fn compute(
    results: &[BenchmarkResult],
    reference: usize,
    estimator: Estimator,
) -> Option<Vec<NormBenchmark<'_>>> {
    // Each command's wall-clock (center, stddev), computed once.
    let summaries: Vec<(f64, Option<f64>)> = results
        .iter()
        .map(|r| {
            let mut times = r.times(|x| x.wall_clock);
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
            let stddev = match (stddev, ref_stddev) {
                (Some(stddev), Some(ref_stddev)) => {
                    Some(stats::ratio_stddev(center, stddev, ref_center, ref_stddev))
                }
                _ => None,
            };
            NormBenchmark {
                result,
                center,
                ratio,
                stddev,
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
        let rel = compute(&results, 0, Estimator::Mean).unwrap();
        assert!(rel[0].is_reference);
        assert!(!rel[1].is_reference);
        assert!((rel[0].ratio - 1.0).abs() < 1e-12);
        assert!((rel[1].ratio - 2.0).abs() < 1e-12);
    }

    #[test]
    fn ratios_against_chosen_reference() {
        let results = [result("a", &[0.1, 0.1]), result("b", &[0.2, 0.2])];
        let rel = compute(&results, 1, Estimator::Mean).unwrap();
        assert!(!rel[0].is_reference);
        assert!(rel[1].is_reference);
        assert!((rel[0].ratio - 0.5).abs() < 1e-12);
        assert!((rel[1].ratio - 1.0).abs() < 1e-12);
    }

    #[test]
    fn reference_out_of_range_is_none() {
        let results = [result("a", &[0.1]), result("b", &[0.2])];
        assert!(compute(&results, 2, Estimator::Mean).is_none());
    }

    #[test]
    fn zero_reference_mean_is_none() {
        let results = [result("a", &[0.0, 0.0]), result("b", &[0.2, 0.2])];
        assert!(compute(&results, 0, Estimator::Mean).is_none());
    }

    #[test]
    fn percentile_estimator_changes_ratio() {
        // b's median is 1.0 (ratio 1) but its mean is 4.0 (ratio 4).
        let results = [
            result("a", &[1.0, 1.0, 1.0]),
            result("b", &[1.0, 1.0, 10.0]),
        ];
        let rel = compute(&results, 0, Estimator::Percentile(50.0)).unwrap();
        assert!((rel[1].ratio - 1.0).abs() < 1e-12);
        let rel = compute(&results, 0, Estimator::Mean).unwrap();
        assert!((rel[1].ratio - 4.0).abs() < 1e-12);
    }

    #[test]
    fn single_command_has_unit_ratio_and_no_stddev() {
        let results = [result("a", &[0.1])];
        let rel = compute(&results, 0, Estimator::Mean).unwrap();
        assert_eq!(rel.len(), 1);
        assert!(rel[0].is_reference);
        assert!((rel[0].ratio - 1.0).abs() < 1e-12);
        assert!(rel[0].stddev.is_none());
    }
}
