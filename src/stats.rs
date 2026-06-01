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
