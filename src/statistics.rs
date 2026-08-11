//! Descriptive statistics for reconstructed LOB state.
//!
//! This module owns measurement only. Feature normalization fit/apply policy
//! belongs to the feature extractor, where the fitted population and feature
//! layout can be bound explicitly.
//!
//! # Key Features
//!
//! - **DayStats**: Per-day statistics for price, size, spread, etc.
//! - **RunningStats**: Online algorithm for incremental mean/std computation
//! - **SessionStats**: Multi-day statistics accumulator
//!
//! # Usage
//!
//! ```ignore
//! use mbo_lob_reconstructor::statistics::{DayStats, RunningStats};
//!
//! let mut stats = DayStats::new("2025-02-03");
//!
//! // Update with each LOB snapshot
//! stats.update(&lob_state);
//!
//! // Get final statistics
//! let summary = stats.finalize();
//! println!("Price mean: {}, std: {}", summary.price_mean, summary.price_std);
//! ```

use crate::constants::{NANODOLLARS_PER_DOLLAR_F64, NS_PER_SECOND_F64};
use crate::types::LobState;
use serde::{Deserialize, Serialize};

// ============================================================================
// Running Statistics (Welford's Algorithm)
// ============================================================================

/// Online algorithm for computing running mean and standard deviation.
///
/// Uses Welford's algorithm for numerical stability with large datasets.
/// This avoids the need to store all values in memory.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RunningStats {
    /// Number of observations
    pub count: u64,
    /// Running mean
    pub mean: f64,
    /// Running M2 (sum of squared differences from mean)
    m2: f64,
    /// Minimum value observed
    pub min: f64,
    /// Maximum value observed
    pub max: f64,
}

impl RunningStats {
    /// Create a new running statistics tracker.
    pub fn new() -> Self {
        Self {
            count: 0,
            mean: 0.0,
            m2: 0.0,
            min: f64::INFINITY,
            max: f64::NEG_INFINITY,
        }
    }

    /// Update statistics with a new value (Welford's online algorithm).
    #[inline]
    pub fn update(&mut self, value: f64) {
        if !value.is_finite() {
            return; // Skip NaN/Inf values
        }

        self.count += 1;
        let delta = value - self.mean;
        self.mean += delta / self.count as f64;
        let delta2 = value - self.mean;
        self.m2 += delta * delta2;

        self.min = self.min.min(value);
        self.max = self.max.max(value);
    }

    /// Get the population variance.
    #[inline]
    pub fn variance(&self) -> f64 {
        if self.count < 2 {
            0.0
        } else {
            self.m2 / self.count as f64
        }
    }

    /// Get the sample variance.
    #[inline]
    pub fn sample_variance(&self) -> f64 {
        if self.count < 2 {
            0.0
        } else {
            self.m2 / (self.count - 1) as f64
        }
    }

    /// Get the population standard deviation.
    #[inline]
    pub fn std(&self) -> f64 {
        self.variance().sqrt()
    }

    /// Get the sample standard deviation.
    #[inline]
    pub fn sample_std(&self) -> f64 {
        self.sample_variance().sqrt()
    }

    /// Merge another RunningStats into this one (parallel algorithm).
    ///
    /// This is useful for combining statistics from multiple threads or days.
    pub fn merge(&mut self, other: &RunningStats) {
        if other.count == 0 {
            return;
        }
        if self.count == 0 {
            *self = other.clone();
            return;
        }

        let combined_count = self.count + other.count;
        let delta = other.mean - self.mean;

        let combined_mean = self.mean + delta * (other.count as f64 / combined_count as f64);
        let combined_m2 = self.m2
            + other.m2
            + delta * delta * (self.count as f64 * other.count as f64 / combined_count as f64);

        self.count = combined_count;
        self.mean = combined_mean;
        self.m2 = combined_m2;
        self.min = self.min.min(other.min);
        self.max = self.max.max(other.max);
    }

    /// Reset statistics to initial state.
    pub fn reset(&mut self) {
        *self = Self::new();
    }

    /// Check if any values have been recorded.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }
}

// ============================================================================
// Day Statistics
// ============================================================================

/// Statistics for a single trading day.
///
/// Tracks all the statistics needed for normalization, including:
/// - Price statistics (for each LOB level)
/// - Size statistics (for each LOB level)
/// - Spread statistics
/// - Volume statistics
/// - Data quality metrics
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DayStats {
    /// Identifier for this day (e.g., "2025-02-03")
    pub date: String,

    /// Statistics for mid-price
    pub mid_price: RunningStats,

    /// Statistics for spread
    pub spread: RunningStats,

    /// Statistics for spread in basis points
    pub spread_bps: RunningStats,

    /// Statistics for microprice
    pub microprice: RunningStats,

    /// Statistics for best bid price
    pub best_bid_price: RunningStats,

    /// Statistics for best ask price
    pub best_ask_price: RunningStats,

    /// Statistics for best bid size
    pub best_bid_size: RunningStats,

    /// Statistics for best ask size
    pub best_ask_size: RunningStats,

    /// Statistics for total bid volume
    pub total_bid_volume: RunningStats,

    /// Statistics for total ask volume
    pub total_ask_volume: RunningStats,

    /// Statistics for depth imbalance
    pub depth_imbalance: RunningStats,

    /// Number of valid LOB snapshots processed
    pub valid_snapshots: u64,

    /// Number of empty LOB snapshots (no quotes)
    pub empty_snapshots: u64,

    /// Number of crossed quote occurrences
    pub crossed_quotes: u64,

    /// Number of locked quote occurrences
    pub locked_quotes: u64,

    /// Total messages processed
    pub total_messages: u64,

    /// First timestamp in the day (nanoseconds)
    pub first_timestamp: Option<i64>,

    /// Last timestamp in the day (nanoseconds)
    pub last_timestamp: Option<i64>,
}

impl DayStats {
    /// Create a new DayStats for the given date.
    pub fn new(date: impl Into<String>) -> Self {
        Self {
            date: date.into(),
            mid_price: RunningStats::new(),
            spread: RunningStats::new(),
            spread_bps: RunningStats::new(),
            microprice: RunningStats::new(),
            best_bid_price: RunningStats::new(),
            best_ask_price: RunningStats::new(),
            best_bid_size: RunningStats::new(),
            best_ask_size: RunningStats::new(),
            total_bid_volume: RunningStats::new(),
            total_ask_volume: RunningStats::new(),
            depth_imbalance: RunningStats::new(),
            valid_snapshots: 0,
            empty_snapshots: 0,
            crossed_quotes: 0,
            locked_quotes: 0,
            total_messages: 0,
            first_timestamp: None,
            last_timestamp: None,
        }
    }

    /// Update statistics with a LOB state snapshot.
    pub fn update(&mut self, state: &LobState) {
        self.total_messages += 1;

        // Track timestamps
        if let Some(ts) = state.timestamp {
            if self.first_timestamp.is_none() {
                self.first_timestamp = Some(ts);
            }
            self.last_timestamp = Some(ts);
        }

        // Check book consistency
        match state.check_consistency() {
            crate::types::BookConsistency::Valid => {
                self.valid_snapshots += 1;
                self.update_valid_state(state);
            }
            crate::types::BookConsistency::Empty => {
                self.empty_snapshots += 1;
            }
            crate::types::BookConsistency::Crossed => {
                self.crossed_quotes += 1;
            }
            crate::types::BookConsistency::Locked => {
                self.locked_quotes += 1;
            }
        }
    }

    /// Update statistics for a valid (non-crossed) LOB state.
    fn update_valid_state(&mut self, state: &LobState) {
        // Mid-price
        if let Some(mid) = state.mid_price() {
            self.mid_price.update(mid);
        }

        // Spread
        if let Some(spread) = state.spread() {
            self.spread.update(spread);
        }

        // Spread in bps
        if let Some(spread_bps) = state.spread_bps() {
            self.spread_bps.update(spread_bps);
        }

        // Microprice
        if let Some(microprice) = state.microprice() {
            self.microprice.update(microprice);
        }

        // Best bid price
        if let Some(bid) = state.best_bid {
            self.best_bid_price
                .update(bid as f64 / NANODOLLARS_PER_DOLLAR_F64);
        }

        // Best ask price
        if let Some(ask) = state.best_ask {
            self.best_ask_price
                .update(ask as f64 / NANODOLLARS_PER_DOLLAR_F64);
        }

        // Best bid size
        if let Some(&size) = state.bid_sizes.first() {
            if size > 0 {
                self.best_bid_size.update(size as f64);
            }
        }

        // Best ask size
        if let Some(&size) = state.ask_sizes.first() {
            if size > 0 {
                self.best_ask_size.update(size as f64);
            }
        }

        // Total volumes
        let bid_vol = state.total_bid_volume();
        if bid_vol > 0 {
            self.total_bid_volume.update(bid_vol as f64);
        }

        let ask_vol = state.total_ask_volume();
        if ask_vol > 0 {
            self.total_ask_volume.update(ask_vol as f64);
        }

        // Depth imbalance
        if let Some(imbalance) = state.depth_imbalance() {
            self.depth_imbalance.update(imbalance);
        }
    }

    /// Get the data quality ratio (valid snapshots / total messages).
    #[inline]
    pub fn data_quality_ratio(&self) -> f64 {
        if self.total_messages == 0 {
            0.0
        } else {
            self.valid_snapshots as f64 / self.total_messages as f64
        }
    }

    /// Get the crossed quote ratio.
    #[inline]
    pub fn crossed_quote_ratio(&self) -> f64 {
        if self.total_messages == 0 {
            0.0
        } else {
            self.crossed_quotes as f64 / self.total_messages as f64
        }
    }

    /// Get the duration of the trading session in seconds.
    pub fn duration_seconds(&self) -> Option<f64> {
        match (self.first_timestamp, self.last_timestamp) {
            (Some(first), Some(last)) => Some((last - first) as f64 / NS_PER_SECOND_F64),
            _ => None,
        }
    }

    /// Merge another DayStats into this one.
    ///
    /// Useful for combining statistics from multiple processing runs.
    pub fn merge(&mut self, other: &DayStats) {
        self.mid_price.merge(&other.mid_price);
        self.spread.merge(&other.spread);
        self.spread_bps.merge(&other.spread_bps);
        self.microprice.merge(&other.microprice);
        self.best_bid_price.merge(&other.best_bid_price);
        self.best_ask_price.merge(&other.best_ask_price);
        self.best_bid_size.merge(&other.best_bid_size);
        self.best_ask_size.merge(&other.best_ask_size);
        self.total_bid_volume.merge(&other.total_bid_volume);
        self.total_ask_volume.merge(&other.total_ask_volume);
        self.depth_imbalance.merge(&other.depth_imbalance);

        self.valid_snapshots += other.valid_snapshots;
        self.empty_snapshots += other.empty_snapshots;
        self.crossed_quotes += other.crossed_quotes;
        self.locked_quotes += other.locked_quotes;
        self.total_messages += other.total_messages;

        // Update timestamps
        if let Some(other_first) = other.first_timestamp {
            match self.first_timestamp {
                Some(self_first) => {
                    self.first_timestamp = Some(self_first.min(other_first));
                }
                None => {
                    self.first_timestamp = Some(other_first);
                }
            }
        }
        if let Some(other_last) = other.last_timestamp {
            match self.last_timestamp {
                Some(self_last) => {
                    self.last_timestamp = Some(self_last.max(other_last));
                }
                None => {
                    self.last_timestamp = Some(other_last);
                }
            }
        }
    }

    /// Create a summary of this day's statistics for logging/display.
    pub fn summary(&self) -> String {
        format!(
            "DayStats for {}:\n  \
            Valid snapshots: {} ({:.2}%)\n  \
            Empty: {}, Crossed: {}, Locked: {}\n  \
            Mid-price: mean={:.4}, std={:.4}, min={:.4}, max={:.4}\n  \
            Spread (bps): mean={:.2}, std={:.2}, min={:.2}, max={:.2}\n  \
            Imbalance: mean={:.4}, std={:.4}",
            self.date,
            self.valid_snapshots,
            self.data_quality_ratio() * 100.0,
            self.empty_snapshots,
            self.crossed_quotes,
            self.locked_quotes,
            self.mid_price.mean,
            self.mid_price.std(),
            self.mid_price.min,
            self.mid_price.max,
            self.spread_bps.mean,
            self.spread_bps.std(),
            self.spread_bps.min,
            self.spread_bps.max,
            self.depth_imbalance.mean,
            self.depth_imbalance.std(),
        )
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // -------------------------------------------------------------------------
    // RunningStats tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_running_stats_new() {
        let stats = RunningStats::new();
        assert_eq!(stats.count, 0);
        assert_eq!(stats.mean, 0.0);
        assert!(stats.is_empty());
    }

    #[test]
    fn test_running_stats_single_value() {
        let mut stats = RunningStats::new();
        stats.update(10.0);

        assert_eq!(stats.count, 1);
        assert_eq!(stats.mean, 10.0);
        assert_eq!(stats.min, 10.0);
        assert_eq!(stats.max, 10.0);
        assert_eq!(stats.variance(), 0.0);
    }

    #[test]
    fn test_running_stats_multiple_values() {
        let mut stats = RunningStats::new();
        let values = [2.0, 4.0, 4.0, 4.0, 5.0, 5.0, 7.0, 9.0];

        for v in &values {
            stats.update(*v);
        }

        assert_eq!(stats.count, 8);
        assert!((stats.mean - 5.0).abs() < 1e-10);
        assert_eq!(stats.min, 2.0);
        assert_eq!(stats.max, 9.0);

        // Population variance = 4.0
        assert!((stats.variance() - 4.0).abs() < 1e-10);
        assert!((stats.std() - 2.0).abs() < 1e-10);
    }

    #[test]
    fn test_running_stats_skip_nan() {
        let mut stats = RunningStats::new();
        stats.update(10.0);
        stats.update(f64::NAN);
        stats.update(20.0);

        assert_eq!(stats.count, 2);
        assert!((stats.mean - 15.0).abs() < 1e-10);
    }

    #[test]
    fn test_running_stats_merge() {
        let mut stats1 = RunningStats::new();
        stats1.update(1.0);
        stats1.update(2.0);
        stats1.update(3.0);

        let mut stats2 = RunningStats::new();
        stats2.update(4.0);
        stats2.update(5.0);
        stats2.update(6.0);

        stats1.merge(&stats2);

        assert_eq!(stats1.count, 6);
        assert!((stats1.mean - 3.5).abs() < 1e-10);
        assert_eq!(stats1.min, 1.0);
        assert_eq!(stats1.max, 6.0);
    }

    #[test]
    fn test_running_stats_merge_empty() {
        let mut stats1 = RunningStats::new();
        stats1.update(5.0);

        let stats2 = RunningStats::new();
        stats1.merge(&stats2);

        assert_eq!(stats1.count, 1);
        assert_eq!(stats1.mean, 5.0);
    }

    // -------------------------------------------------------------------------
    // DayStats tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_day_stats_new() {
        let stats = DayStats::new("2025-02-03");
        assert_eq!(stats.date, "2025-02-03");
        assert_eq!(stats.valid_snapshots, 0);
        assert_eq!(stats.total_messages, 0);
    }

    #[test]
    fn test_day_stats_update_valid() {
        let mut stats = DayStats::new("2025-02-03");

        let mut state = LobState::new(10);
        state.best_bid = Some(100_000_000_000); // $100.00
        state.best_ask = Some(100_010_000_000); // $100.01
        state.bid_sizes[0] = 100;
        state.ask_sizes[0] = 200;
        state.timestamp = Some(1_234_567_890_000_000_000);

        stats.update(&state);

        assert_eq!(stats.valid_snapshots, 1);
        assert_eq!(stats.total_messages, 1);
        assert_eq!(stats.empty_snapshots, 0);
        assert_eq!(stats.crossed_quotes, 0);
        assert!(stats.mid_price.count > 0);
    }

    #[test]
    fn test_day_stats_update_empty() {
        let mut stats = DayStats::new("2025-02-03");
        let state = LobState::new(10); // Empty state

        stats.update(&state);

        assert_eq!(stats.empty_snapshots, 1);
        assert_eq!(stats.valid_snapshots, 0);
    }

    #[test]
    fn test_day_stats_update_crossed() {
        let mut stats = DayStats::new("2025-02-03");

        let mut state = LobState::new(10);
        state.best_bid = Some(100_010_000_000); // $100.01 (bid > ask!)
        state.best_ask = Some(100_000_000_000); // $100.00

        stats.update(&state);

        assert_eq!(stats.crossed_quotes, 1);
        assert_eq!(stats.valid_snapshots, 0);
    }

    #[test]
    fn test_day_stats_data_quality_ratio() {
        let mut stats = DayStats::new("2025-02-03");

        // Add 3 valid snapshots
        let mut valid_state = LobState::new(10);
        valid_state.best_bid = Some(100_000_000_000);
        valid_state.best_ask = Some(100_010_000_000);
        valid_state.bid_sizes[0] = 100;
        valid_state.ask_sizes[0] = 100;

        for _ in 0..3 {
            stats.update(&valid_state);
        }

        // Add 1 empty snapshot
        let empty_state = LobState::new(10);
        stats.update(&empty_state);

        // Add 1 crossed snapshot
        let mut crossed_state = LobState::new(10);
        crossed_state.best_bid = Some(100_010_000_000);
        crossed_state.best_ask = Some(100_000_000_000);
        stats.update(&crossed_state);

        assert_eq!(stats.total_messages, 5);
        assert_eq!(stats.valid_snapshots, 3);
        assert!((stats.data_quality_ratio() - 0.6).abs() < 1e-10);
    }

    #[test]
    fn test_day_stats_summary() {
        let mut stats = DayStats::new("2025-02-03");

        let mut state = LobState::new(10);
        state.best_bid = Some(100_000_000_000);
        state.best_ask = Some(100_010_000_000);
        state.bid_sizes[0] = 100;
        state.ask_sizes[0] = 100;

        stats.update(&state);

        let summary = stats.summary();
        assert!(summary.contains("2025-02-03"));
        assert!(summary.contains("Valid snapshots: 1"));
    }
}
