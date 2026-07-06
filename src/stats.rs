pub fn min(values: &[f64]) -> f64 {
    values.iter().copied().fold(f64::INFINITY, f64::min)
}

pub fn max(values: &[f64]) -> f64 {
    values.iter().copied().fold(f64::NEG_INFINITY, f64::max)
}

pub fn mean(values: &[f64]) -> f64 {
    let r = values.iter().fold(0.0, |acc, x| acc + x);
    match values.len() {
        0 => 0.,
        n => r / n as f64,
    }
}

/// The p-th percentile (0..=100) of already-**sorted** values, interpolating
/// linearly between adjacent ranks; 0.0 for an empty slice.
pub fn percentile(sorted: &[f64], p: f64) -> f64 {
    match sorted.len() {
        0 => 0.0,
        n => {
            let rank = p / 100.0 * (n - 1) as f64;
            let lo = rank.floor() as usize;
            let hi = rank.ceil() as usize;
            sorted[lo] + (sorted[hi] - sorted[lo]) * (rank - lo as f64)
        }
    }
}

/// The statistic reported as a command's central time (`--estimator`).
#[derive(Clone, Copy, PartialEq)]
pub enum Estimator {
    Mean,
    /// Percentile in [0, 100]; the median is `Percentile(50.0)`.
    Percentile(f64),
}

impl Estimator {
    /// `mean`, `median`, or `p` + digits where digits past the second land
    /// after the decimal point: `p90` → 90, `p999` → 99.9, `p9995` → 99.95
    /// (integers up to 100 are taken as-is, so `p100` is the maximum).
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "mean" => Some(Estimator::Mean),
            "median" => Some(Estimator::Percentile(50.0)),
            _ => {
                let digits = s.strip_prefix('p')?;
                if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
                    return None;
                }
                let p = match digits.parse::<u32>() {
                    Ok(n) if n <= 100 => n as f64,
                    _ => format!("{}.{}", &digits[..2], &digits[2..]).parse().ok()?,
                };
                Some(Estimator::Percentile(p))
            }
        }
    }

    /// The central value of already-**sorted** samples under this estimator.
    pub fn value(self, sorted: &[f64]) -> f64 {
        match self {
            Estimator::Mean => mean(sorted),
            Estimator::Percentile(p) => percentile(sorted, p),
        }
    }
}

impl std::fmt::Display for Estimator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Estimator::Mean => write!(f, "mean"),
            Estimator::Percentile(p) if *p == 50.0 => write!(f, "median"),
            Estimator::Percentile(p) => write!(f, "p{p}"),
        }
    }
}

/// Sample mean and standard deviation in one pass over the data. The stddev is
/// `None` for fewer than two values.
pub fn mean_stddev(values: &[f64]) -> (f64, Option<f64>) {
    let m = mean(values);
    if values.len() < 2 {
        return (m, None);
    }
    let variance = values.iter().map(|x| (x - m).powi(2)).sum::<f64>() / (values.len() - 1) as f64;
    (m, Some(variance.sqrt()))
}

/// Welch's t statistic and Welch–Satterthwaite degrees of freedom for two
/// samples summarized as (mean, stddev, count).
pub fn welch_t(m1: f64, s1: f64, n1: usize, m2: f64, s2: f64, n2: usize) -> (f64, f64) {
    let v1 = s1 * s1 / n1 as f64;
    let v2 = s2 * s2 / n2 as f64;
    let t = (m1 - m2) / (v1 + v2).sqrt();
    let df = (v1 + v2).powi(2) / (v1 * v1 / (n1 - 1) as f64 + v2 * v2 / (n2 - 1) as f64);
    (t, df)
}

/// Two-sided p-value of a t statistic with `df` degrees of freedom:
/// `I_x(df/2, 1/2)` at `x = df/(df + t²)` (the Student-t survival identity).
pub fn t_test_p(t: f64, df: f64) -> f64 {
    betainc(df / 2.0, 0.5, df / (df + t * t))
}

/// ln Γ(x) via the 6-coefficient Lanczos approximation (Numerical Recipes
/// `gammln`), accurate to ~1e-10 for x > 0.
fn ln_gamma(x: f64) -> f64 {
    const COF: [f64; 6] = [
        76.18009172947146,
        -86.50532032941677,
        24.01409824083091,
        -1.231739572450155,
        0.1208650973866179e-2,
        -0.5395239384953e-5,
    ];
    let tmp = x + 5.5;
    let tmp = tmp - (x + 0.5) * tmp.ln();
    let mut y = x;
    let mut ser = 1.000000000190015;
    for c in COF {
        y += 1.0;
        ser += c / y;
    }
    -tmp + (2.5066282746310005 * ser / x).ln()
}

/// Continued fraction of the incomplete beta (Numerical Recipes `betacf`,
/// modified Lentz's method).
fn betacf(a: f64, b: f64, x: f64) -> f64 {
    const EPS: f64 = 3e-12;
    const FPMIN: f64 = 1e-30;
    let (qab, qap, qam) = (a + b, a + 1.0, a - 1.0);
    let mut c = 1.0;
    let mut d = 1.0 - qab * x / qap;
    if d.abs() < FPMIN {
        d = FPMIN;
    }
    d = 1.0 / d;
    let mut h = d;
    for m in 1..=200 {
        let m = m as f64;
        let m2 = 2.0 * m;
        let aa = m * (b - m) * x / ((qam + m2) * (a + m2));
        d = 1.0 + aa * d;
        if d.abs() < FPMIN {
            d = FPMIN;
        }
        c = 1.0 + aa / c;
        if c.abs() < FPMIN {
            c = FPMIN;
        }
        d = 1.0 / d;
        h *= d * c;
        let aa = -(a + m) * (qab + m) * x / ((a + m2) * (qap + m2));
        d = 1.0 + aa * d;
        if d.abs() < FPMIN {
            d = FPMIN;
        }
        c = 1.0 + aa / c;
        if c.abs() < FPMIN {
            c = FPMIN;
        }
        d = 1.0 / d;
        let del = d * c;
        h *= del;
        if (del - 1.0).abs() < EPS {
            break;
        }
    }
    h
}

/// Regularized incomplete beta `I_x(a, b)` (Numerical Recipes `betai`).
fn betainc(a: f64, b: f64, x: f64) -> f64 {
    if x <= 0.0 {
        return 0.0;
    }
    if x >= 1.0 {
        return 1.0;
    }
    let front =
        (ln_gamma(a + b) - ln_gamma(a) - ln_gamma(b) + a * x.ln() + b * (1.0 - x).ln()).exp();
    if x < (a + 1.0) / (a + b + 2.0) {
        front * betacf(a, b, x) / a
    } else {
        1.0 - front * betacf(b, a, 1.0 - x) / b
    }
}

/// Propagated uncertainty on the ratio `mean / ref_mean` of two measured means,
/// assuming zero covariance (same formula as hyperfine, for f = A/B):
/// https://en.wikipedia.org/wiki/Propagation_of_uncertainty#Example_formulae
pub fn ratio_stddev(mean: f64, stddev: f64, ref_mean: f64, ref_stddev: f64) -> f64 {
    (mean / ref_mean) * ((stddev / mean).powi(2) + (ref_stddev / ref_mean).powi(2)).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mean_of_values() {
        assert_eq!(mean(&[1.0, 2.0, 3.0]), 2.0);
        assert_eq!(mean(&[]), 0.0);
        assert_eq!(mean(&[5.0]), 5.0);
    }

    #[test]
    fn min_max() {
        assert_eq!(min(&[3.0, 1.0, 2.0]), 1.0);
        assert_eq!(max(&[3.0, 1.0, 2.0]), 3.0);
    }

    #[test]
    fn percentile_interpolates() {
        let v = [1.0, 2.0, 3.0, 4.0];
        assert_eq!(percentile(&v, 0.0), 1.0);
        assert_eq!(percentile(&v, 100.0), 4.0);
        assert_eq!(percentile(&v, 50.0), 2.5);
        assert!((percentile(&v, 90.0) - 3.7).abs() < 1e-12);
        assert_eq!(percentile(&[], 50.0), 0.0);
        assert_eq!(percentile(&[7.0], 99.0), 7.0);
    }

    #[test]
    fn estimator_parse() {
        // Estimator has no Debug, so compare with `==` rather than assert_eq!.
        assert!(Estimator::parse("mean") == Some(Estimator::Mean));
        assert!(Estimator::parse("median") == Some(Estimator::Percentile(50.0)));
        assert!(Estimator::parse("p0") == Some(Estimator::Percentile(0.0)));
        assert!(Estimator::parse("p90") == Some(Estimator::Percentile(90.0)));
        assert!(Estimator::parse("p100") == Some(Estimator::Percentile(100.0)));
        assert!(Estimator::parse("p999") == Some(Estimator::Percentile(99.9)));
        assert!(Estimator::parse("p9995") == Some(Estimator::Percentile(99.95)));
        assert!(Estimator::parse("p").is_none());
        assert!(Estimator::parse("p12.5").is_none());
        assert!(Estimator::parse("avg").is_none());
    }

    #[test]
    fn estimator_labels() {
        assert_eq!(Estimator::Mean.to_string(), "mean");
        assert_eq!(Estimator::Percentile(50.0).to_string(), "median");
        assert_eq!(Estimator::Percentile(90.0).to_string(), "p90");
        assert_eq!(Estimator::Percentile(99.9).to_string(), "p99.9");
    }

    #[test]
    fn stddev_needs_two_values() {
        assert_eq!(mean_stddev(&[]), (0.0, None));
        assert_eq!(mean_stddev(&[5.0]), (5.0, None));
    }

    #[test]
    fn ratio_stddev_propagation() {
        // f = A/B with A = 2 ± 0.2, B = 1 ± 0.1: σ_f = 2·√(0.01 + 0.01).
        assert!((ratio_stddev(2.0, 0.2, 1.0, 0.1) - 0.282_842_712).abs() < 1e-6);
    }

    #[test]
    fn welch_known_values() {
        // scipy.stats.ttest_ind_from_stats(10, 1, 10, 11, 1, 10, equal_var=False)
        let (t, df) = welch_t(10.0, 1.0, 10, 11.0, 1.0, 10);
        assert!((t + 2.236_068).abs() < 1e-5, "t = {t}");
        assert!((df - 18.0).abs() < 1e-9, "df = {df}");
    }

    #[test]
    fn t_test_p_known_values() {
        assert!((t_test_p(0.0, 10.0) - 1.0).abs() < 1e-9);
        // df=1 is the Cauchy distribution: P(|T| > 1) = 0.5 exactly.
        assert!((t_test_p(1.0, 1.0) - 0.5).abs() < 1e-9);
        // scipy: 2 * t.sf(2, 10) and 2 * t.sf(2.236068, 18).
        assert!((t_test_p(2.0, 10.0) - 0.073_388).abs() < 1e-3);
        assert!((t_test_p(2.236_068, 18.0) - 0.038_250).abs() < 1e-3);
    }

    #[test]
    fn stddev_known_set() {
        // Sample stddev (n-1) of {2,4,4,4,5,5,7,9} is 2.0, mean 5.0.
        let (m, s) = mean_stddev(&[2.0, 4.0, 4.0, 4.0, 5.0, 5.0, 7.0, 9.0]);
        assert_eq!(m, 5.0);
        assert!((s.unwrap() - 2.138_089_9).abs() < 1e-6);
    }
}
