use std::process::ExitStatus;

use crate::stats;

/// The complete outcome of one command execution: its exit status, wall-clock
/// time, and the resource usage reported by `wait4`. Only successful runs are
/// kept in a `BenchmarkResult` (a non-zero exit is surfaced as an error instead).
pub struct Execution {
    /// Exit status of the process.
    pub status: ExitStatus,
    /// Elapsed wall clock time, in seconds.
    pub wall_clock: f64,
    /// Time spent in user mode, in seconds.
    pub time_user: f64,
    /// Time spent in kernel mode, in seconds.
    pub time_system: f64,
    /// Peak resident set size, in bytes.
    pub max_rss: u64,
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
    pub mean: f64,
    /// `mean / reference_mean` (the first command's mean).
    pub ratio: f64,
    /// Propagated uncertainty on `ratio`, if both stddevs are known.
    pub stddev: Option<f64>,
    pub is_reference: bool,
}

/// Compare every result against the `reference`-th one. Returns `None` if the
/// reference mean time is zero (relative speed would be meaningless). The caller
/// guarantees `reference < results.len()`.
pub fn compute(results: &[BenchmarkResult], reference: usize) -> Option<Vec<NormBenchmark<'_>>> {
    // Each command's wall-clock (mean, stddev), computed once.
    let summaries: Vec<(f64, Option<f64>)> = results
        .iter()
        .map(|r| stats::mean_stddev(&r.times(|x| x.wall_clock)))
        .collect();

    let (ref_mean, ref_stddev) = *summaries.get(reference)?;
    if ref_mean == 0.0 {
        return None;
    }

    let relative = results
        .iter()
        .zip(&summaries)
        .enumerate()
        .map(|(idx, (result, &(mean, stddev)))| {
            let ratio = mean / ref_mean;
            let stddev = match (stddev, ref_stddev) {
                (Some(stddev), Some(ref_stddev)) => {
                    // same formula than hyperfine:
                    // https://en.wikipedia.org/wiki/Propagation_of_uncertainty#Example_formulae
                    // for f=A/B
                    // with σ_{AB} assumed to be = 0

                    Some(ratio * ((stddev / mean).powi(2) + (ref_stddev / ref_mean).powi(2)).sqrt())
                }
                _ => None,
            };
            NormBenchmark {
                result,
                mean,
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
    use std::os::unix::process::ExitStatusExt;

    /// A `BenchmarkResult` with the given wall-clock samples (other fields unused
    /// by `compute`).
    fn result(label: &str, wall_clocks: &[f64]) -> BenchmarkResult {
        BenchmarkResult {
            label: label.to_string(),
            measurements: wall_clocks
                .iter()
                .map(|&wall_clock| Execution {
                    status: ExitStatus::from_raw(0),
                    wall_clock,
                    time_user: 0.0,
                    time_system: 0.0,
                    max_rss: 0,
                })
                .collect(),
        }
    }

    #[test]
    fn ratios_against_first() {
        let results = [result("a", &[0.1, 0.1]), result("b", &[0.2, 0.2])];
        let rel = compute(&results, 0).unwrap();
        assert!(rel[0].is_reference);
        assert!(!rel[1].is_reference);
        assert!((rel[0].ratio - 1.0).abs() < 1e-12);
        assert!((rel[1].ratio - 2.0).abs() < 1e-12);
    }

    #[test]
    fn ratios_against_chosen_reference() {
        let results = [result("a", &[0.1, 0.1]), result("b", &[0.2, 0.2])];
        let rel = compute(&results, 1).unwrap();
        assert!(!rel[0].is_reference);
        assert!(rel[1].is_reference);
        assert!((rel[0].ratio - 0.5).abs() < 1e-12);
        assert!((rel[1].ratio - 1.0).abs() < 1e-12);
    }

    #[test]
    fn reference_out_of_range_is_none() {
        let results = [result("a", &[0.1]), result("b", &[0.2])];
        assert!(compute(&results, 2).is_none());
    }

    #[test]
    fn zero_reference_mean_is_none() {
        let results = [result("a", &[0.0, 0.0]), result("b", &[0.2, 0.2])];
        assert!(compute(&results, 0).is_none());
    }

    #[test]
    fn single_command_has_unit_ratio_and_no_stddev() {
        let results = [result("a", &[0.1])];
        let rel = compute(&results, 0).unwrap();
        assert_eq!(rel.len(), 1);
        assert!(rel[0].is_reference);
        assert!((rel[0].ratio - 1.0).abs() < 1e-12);
        assert!(rel[0].stddev.is_none());
    }
}
