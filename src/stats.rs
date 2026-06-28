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
    fn stddev_needs_two_values() {
        assert_eq!(mean_stddev(&[]), (0.0, None));
        assert_eq!(mean_stddev(&[5.0]), (5.0, None));
    }

    #[test]
    fn stddev_known_set() {
        // Sample stddev (n-1) of {2,4,4,4,5,5,7,9} is 2.0, mean 5.0.
        let (m, s) = mean_stddev(&[2.0, 4.0, 4.0, 4.0, 5.0, 5.0, 7.0, 9.0]);
        assert_eq!(m, 5.0);
        assert!((s.unwrap() - 2.138_089_9).abs() < 1e-6);
    }
}
