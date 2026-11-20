#[derive(Debug, PartialEq, Eq)]
pub struct SpeedStats {
    pub current: u64,
    pub peak: u64,
    pub average: u64,
}

pub fn summarize(current: u64, samples: &[u64]) -> SpeedStats {
    SpeedStats {
        current,
        peak: samples.iter().copied().max().unwrap_or(0).max(current),
        average: if samples.is_empty() {
            0
        } else {
            (samples
                .iter()
                .map(|sample| u128::from(*sample))
                .sum::<u128>()
                / samples.len() as u128) as u64
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speed_statistics_use_current_peak_and_mean_of_available_samples() {
        assert_eq!(
            summarize(250, &[100, 300, 200]),
            SpeedStats {
                current: 250,
                peak: 300,
                average: 200
            }
        );
        assert_eq!(
            summarize(500, &[0, 100, 200]),
            SpeedStats {
                current: 500,
                peak: 500,
                average: 100
            }
        );
    }

    #[test]
    fn empty_history_has_no_average_and_keeps_current_peak() {
        assert_eq!(
            summarize(0, &[]),
            SpeedStats {
                current: 0,
                peak: 0,
                average: 0
            }
        );
        assert_eq!(
            summarize(5, &[]),
            SpeedStats {
                current: 5,
                peak: 5,
                average: 0
            }
        );
    }

    #[test]
    fn statistics_do_not_overflow_large_speed_samples() {
        assert_eq!(summarize(0, &[u64::MAX, u64::MAX]).average, u64::MAX);
    }
}
