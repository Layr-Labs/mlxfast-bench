//! Small, paired-free statistical primitives.
//!
//! Extracted from the retired qwen-mtp-paired-decode-only scoring so a generic, reusable
//! aggregation utility does not travel with (or die with) the paired seam. `even_n_median` makes
//! NO paired / candidate-vs-baseline assumption — it is a plain order-statistic median over a slice
//! of `f64`. A future multi-golden "median-of-per-prompt-gains" mode for the single-leg official
//! path is the intended next consumer.

/// The EVEN-N median of a slice: for an odd count the middle order statistic, for an even count the
/// mean of the two central order statistics. Returns `NaN` for an empty slice (the caller guards
/// non-empty). NaN samples sort last; a materialised copy is sorted, so the input is untouched.
///
/// This is the `even_n_mean_of_two_central_order_statistics` rule (NOT the lower-median rule a p50
/// diagnostic uses).
pub fn even_n_median(samples: &[f64]) -> f64 {
    let n = samples.len();
    if n == 0 {
        return f64::NAN;
    }
    let mut sorted = samples.to_vec();
    // Total order over f64 for the order statistics; NaN sorts last. `partial_cmp` is safe here as
    // we sort a materialised copy.
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Greater));
    if n % 2 == 1 {
        sorted[n / 2]
    } else {
        (sorted[n / 2 - 1] + sorted[n / 2]) / 2.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn even_n_median_odd_is_middle_even_is_mean_of_two_central() {
        // Odd n → middle order statistic.
        assert_eq!(even_n_median(&[0.9, 1.1, 1.0]), 1.0);
        // Even n → mean of the two central order statistics (NOT lower-median).
        assert_eq!(even_n_median(&[1.0, 2.0, 3.0, 4.0]), 2.5);
        // Single sample → that one value.
        assert_eq!(even_n_median(&[1.234]), 1.234);
        // Unsorted input is ordered first.
        assert_eq!(even_n_median(&[4.0, 1.0, 3.0, 2.0]), 2.5);
    }

    #[test]
    fn even_n_median_empty_is_nan() {
        assert!(even_n_median(&[]).is_nan());
    }
}
