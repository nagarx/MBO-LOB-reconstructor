//! Day Boundary Detection for LOB Data Processing
//!
//! This module provides automatic detection of trading day boundaries in MBO data streams.
//! Proper day boundary detection is critical for:
//!
//! - **Train/Test Splits**: All benchmark papers (FI-2010, DeepLOB, LOBCAST) split by trading days
//! - **State Reset**: LOB state should be reset at day boundaries to avoid stale data
//! - **Statistics Reset**: Rolling statistics (normalization, sampling) need day resets
//!
//! # Research Reference
//!
//! - FI-2010: Train on days 1-7, test on days 8-10
//! - DeepLOB: Similar day-based splitting
//! - LOBCAST: Anchor day + rolling window approach
//!
//! # Example
//!
//! ```ignore
//! use mbo_lob_reconstructor::lob::day_boundary::{DayBoundaryConfig, DayBoundaryDetector};
//!
//! let config = DayBoundaryConfig::us_equity();
//! let mut detector = DayBoundaryDetector::new(config);
//!
//! for msg in messages {
//!     if let Some(ts) = msg.timestamp {
//!         if let Some(boundary) = detector.check_boundary(ts) {
//!             println!("Day {} ended, day {} started", 
//!                      boundary.previous_day_index, 
//!                      boundary.new_day_index);
//!             // Reset your state here
//!         }
//!     }
//! }
//! ```

use serde::{Deserialize, Serialize};

// ============================================================================
// Constants
// ============================================================================

/// Nanoseconds per second
const NS_PER_SECOND: i64 = 1_000_000_000;

/// Nanoseconds per minute
const NS_PER_MINUTE: i64 = 60 * NS_PER_SECOND;

/// Nanoseconds per hour
const NS_PER_HOUR: i64 = 60 * NS_PER_MINUTE;

/// Nanoseconds per day
const NS_PER_DAY: i64 = 24 * NS_PER_HOUR;

// ============================================================================
// Configuration
// ============================================================================

/// Configuration for day boundary detection.
///
/// # Presets
///
/// - `us_equity()`: US equity markets (9:30 AM - 4:00 PM ET)
/// - `us_futures()`: US futures markets (extended hours)
/// - `crypto()`: 24/7 cryptocurrency markets (midnight UTC boundary)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DayBoundaryConfig {
    /// Market open time as nanoseconds from midnight UTC.
    ///
    /// Example: 9:30 AM ET = 14:30 UTC = 52_200_000_000_000 ns
    pub market_open_ns: i64,

    /// Market close time as nanoseconds from midnight UTC.
    ///
    /// Example: 4:00 PM ET = 21:00 UTC = 75_600_000_000_000 ns
    pub market_close_ns: i64,

    /// Minimum gap between messages to consider a day boundary (nanoseconds).
    ///
    /// Typical: 4+ hours for overnight gap detection.
    /// Default: 4 hours = 14_400_000_000_000 ns
    pub gap_threshold_ns: i64,

    /// Timezone offset from UTC in hours.
    ///
    /// Example: US Eastern = -5 (or -4 during DST)
    pub timezone_offset_hours: i32,

    /// Whether to use gap detection (true) or fixed time boundaries (false).
    ///
    /// - `true`: Detect gaps > threshold as day boundaries (more flexible)
    /// - `false`: Use fixed market open/close times (more precise)
    pub use_gap_detection: bool,
}

impl DayBoundaryConfig {
    /// Create a new configuration with custom parameters.
    pub fn new(
        market_open_ns: i64,
        market_close_ns: i64,
        gap_threshold_ns: i64,
        timezone_offset_hours: i32,
    ) -> Self {
        Self {
            market_open_ns,
            market_close_ns,
            gap_threshold_ns,
            timezone_offset_hours,
            use_gap_detection: true,
        }
    }

    /// US equity markets preset (NYSE, NASDAQ).
    ///
    /// - Open: 9:30 AM ET (14:30 UTC)
    /// - Close: 4:00 PM ET (21:00 UTC)
    /// - Gap threshold: 4 hours
    pub fn us_equity() -> Self {
        Self {
            // 9:30 AM ET = 14:30 UTC (assuming EST, -5 hours)
            market_open_ns: 14 * NS_PER_HOUR + 30 * NS_PER_MINUTE,
            // 4:00 PM ET = 21:00 UTC
            market_close_ns: 21 * NS_PER_HOUR,
            gap_threshold_ns: 4 * NS_PER_HOUR,
            timezone_offset_hours: -5,
            use_gap_detection: true,
        }
    }

    /// US futures markets preset (CME, extended hours).
    ///
    /// - Gap threshold: 1 hour (shorter due to near 24h trading)
    pub fn us_futures() -> Self {
        Self {
            // CME opens Sunday 5 PM CT, closes Friday 4 PM CT
            // Using midnight UTC as reference for simplicity
            market_open_ns: 0,
            market_close_ns: NS_PER_DAY - 1,
            gap_threshold_ns: 1 * NS_PER_HOUR,
            timezone_offset_hours: -6, // CT
            use_gap_detection: true,
        }
    }

    /// Cryptocurrency markets preset (24/7).
    ///
    /// Uses midnight UTC as day boundary.
    pub fn crypto() -> Self {
        Self {
            market_open_ns: 0,
            market_close_ns: NS_PER_DAY - 1,
            gap_threshold_ns: 0, // No gap detection, use midnight boundary
            timezone_offset_hours: 0,
            use_gap_detection: false,
        }
    }

    /// Enable gap-based detection (default).
    pub fn with_gap_detection(mut self) -> Self {
        self.use_gap_detection = true;
        self
    }

    /// Use fixed time boundaries instead of gap detection.
    pub fn with_fixed_boundaries(mut self) -> Self {
        self.use_gap_detection = false;
        self
    }

    /// Set custom gap threshold.
    pub fn with_gap_threshold_hours(mut self, hours: i64) -> Self {
        self.gap_threshold_ns = hours * NS_PER_HOUR;
        self
    }
}

impl Default for DayBoundaryConfig {
    fn default() -> Self {
        Self::us_equity()
    }
}

// ============================================================================
// Day Boundary Event
// ============================================================================

/// Information about a detected day boundary.
#[derive(Debug, Clone)]
pub struct DayBoundary {
    /// Timestamp of the last message from the previous day.
    pub previous_day_end_ts: i64,

    /// Timestamp of the first message from the new day.
    pub new_day_start_ts: i64,

    /// Gap duration in nanoseconds.
    pub gap_ns: i64,

    /// Index of the previous trading day (0-based).
    pub previous_day_index: u32,

    /// Index of the new trading day (0-based).
    pub new_day_index: u32,

    /// Statistics from the previous day.
    pub previous_day_stats: DayBoundaryStats,
}

impl DayBoundary {
    /// Get the gap duration in seconds.
    pub fn gap_seconds(&self) -> f64 {
        self.gap_ns as f64 / NS_PER_SECOND as f64
    }

    /// Get the gap duration in hours.
    pub fn gap_hours(&self) -> f64 {
        self.gap_ns as f64 / NS_PER_HOUR as f64
    }
}

// ============================================================================
// Day Statistics
// ============================================================================

/// Statistics for a single trading day (message/trade counts).
///
/// This tracks basic metrics for each trading day detected by `DayBoundaryDetector`.
/// For running price/spread statistics, see `mbo_lob_reconstructor::statistics::DayStats`.
#[derive(Debug, Clone, Default)]
pub struct DayBoundaryStats {
    /// Number of messages processed.
    pub messages: u64,

    /// Number of trades.
    pub trades: u64,

    /// First timestamp of the day.
    pub first_ts: Option<i64>,

    /// Last timestamp of the day.
    pub last_ts: Option<i64>,

    /// Total volume traded.
    pub total_volume: u64,

    /// Number of LOB snapshots generated.
    pub snapshots: u64,
}

impl DayBoundaryStats {
    /// Create new empty stats.
    pub fn new() -> Self {
        Self::default()
    }

    /// Update stats with a message.
    pub fn record_message(&mut self, timestamp: Option<i64>, is_trade: bool, size: u32) {
        self.messages += 1;

        if let Some(ts) = timestamp {
            if self.first_ts.is_none() {
                self.first_ts = Some(ts);
            }
            self.last_ts = Some(ts);
        }

        if is_trade {
            self.trades += 1;
            self.total_volume += size as u64;
        }
    }

    /// Record a snapshot generation.
    pub fn record_snapshot(&mut self) {
        self.snapshots += 1;
    }

    /// Get duration in nanoseconds.
    pub fn duration_ns(&self) -> Option<i64> {
        match (self.first_ts, self.last_ts) {
            (Some(first), Some(last)) => Some(last - first),
            _ => None,
        }
    }

    /// Get duration in seconds.
    pub fn duration_seconds(&self) -> Option<f64> {
        self.duration_ns()
            .map(|ns| ns as f64 / NS_PER_SECOND as f64)
    }
}

// ============================================================================
// Day Boundary Detector
// ============================================================================

/// Detects trading day boundaries in timestamp streams.
///
/// # Usage
///
/// Call `check_boundary()` with each message timestamp. When a day boundary
/// is detected, it returns `Some(DayBoundary)` with information about the
/// transition.
///
/// # Example
///
/// ```
/// use mbo_lob_reconstructor::lob::day_boundary::{DayBoundaryConfig, DayBoundaryDetector};
///
/// let mut detector = DayBoundaryDetector::new(DayBoundaryConfig::us_equity());
///
/// // Simulate messages
/// let ts1 = 1704067200_000_000_000i64; // Day 1
/// let ts2 = ts1 + 3600_000_000_000;     // 1 hour later (same day)
/// let ts3 = ts1 + 20 * 3600_000_000_000; // 20 hours later (next day)
///
/// assert!(detector.check_boundary(ts1).is_none()); // First message
/// assert!(detector.check_boundary(ts2).is_none()); // Same day
/// assert!(detector.check_boundary(ts3).is_some()); // Day boundary!
/// ```
pub struct DayBoundaryDetector {
    config: DayBoundaryConfig,

    /// Last seen timestamp.
    last_ts: Option<i64>,

    /// Current trading day index (0-based).
    current_day_index: u32,

    /// Statistics for the current day.
    current_day_stats: DayBoundaryStats,

    /// Total number of boundaries detected.
    boundaries_detected: u32,
}

impl DayBoundaryDetector {
    /// Create a new detector with the given configuration.
    pub fn new(config: DayBoundaryConfig) -> Self {
        Self {
            config,
            last_ts: None,
            current_day_index: 0,
            current_day_stats: DayBoundaryStats::new(),
            boundaries_detected: 0,
        }
    }

    /// Check if a timestamp represents a day boundary.
    ///
    /// Returns `Some(DayBoundary)` if a day boundary is detected,
    /// `None` otherwise.
    pub fn check_boundary(&mut self, timestamp: i64) -> Option<DayBoundary> {
        let boundary = match self.last_ts {
            None => {
                // First message - no boundary
                None
            }
            Some(last_ts) => {
                let gap = timestamp - last_ts;

                let is_boundary = if self.config.use_gap_detection {
                    // Gap-based detection
                    gap > self.config.gap_threshold_ns
                } else {
                    // Fixed time boundary detection
                    self.crosses_midnight(last_ts, timestamp)
                };

                if is_boundary {
                    let previous_stats = std::mem::take(&mut self.current_day_stats);
                    let previous_day = self.current_day_index;
                    self.current_day_index += 1;
                    self.boundaries_detected += 1;

                    Some(DayBoundary {
                        previous_day_end_ts: last_ts,
                        new_day_start_ts: timestamp,
                        gap_ns: gap,
                        previous_day_index: previous_day,
                        new_day_index: self.current_day_index,
                        previous_day_stats: previous_stats,
                    })
                } else {
                    None
                }
            }
        };

        self.last_ts = Some(timestamp);
        boundary
    }

    /// Record a message for statistics (call after check_boundary).
    pub fn record_message(&mut self, timestamp: Option<i64>, is_trade: bool, size: u32) {
        self.current_day_stats
            .record_message(timestamp, is_trade, size);
    }

    /// Record a snapshot generation.
    pub fn record_snapshot(&mut self) {
        self.current_day_stats.record_snapshot();
    }

    /// Get current day index.
    pub fn current_day_index(&self) -> u32 {
        self.current_day_index
    }

    /// Get current day statistics.
    pub fn current_day_stats(&self) -> &DayBoundaryStats {
        &self.current_day_stats
    }

    /// Get total boundaries detected.
    pub fn boundaries_detected(&self) -> u32 {
        self.boundaries_detected
    }

    /// Get the last seen timestamp.
    pub fn last_timestamp(&self) -> Option<i64> {
        self.last_ts
    }

    /// Reset the detector to initial state.
    pub fn reset(&mut self) {
        self.last_ts = None;
        self.current_day_index = 0;
        self.current_day_stats = DayBoundaryStats::new();
        self.boundaries_detected = 0;
    }

    /// Check if two timestamps cross midnight UTC.
    fn crosses_midnight(&self, ts1: i64, ts2: i64) -> bool {
        let day1 = ts1 / NS_PER_DAY;
        let day2 = ts2 / NS_PER_DAY;
        day2 > day1
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_us_equity() {
        let config = DayBoundaryConfig::us_equity();
        assert!(config.use_gap_detection);
        assert_eq!(config.gap_threshold_ns, 4 * NS_PER_HOUR);
        assert_eq!(config.timezone_offset_hours, -5);
    }

    #[test]
    fn test_config_crypto() {
        let config = DayBoundaryConfig::crypto();
        assert!(!config.use_gap_detection);
        assert_eq!(config.gap_threshold_ns, 0);
    }

    #[test]
    fn test_detector_first_message_no_boundary() {
        let mut detector = DayBoundaryDetector::new(DayBoundaryConfig::us_equity());
        let ts = 1704067200_000_000_000i64;
        assert!(detector.check_boundary(ts).is_none());
        assert_eq!(detector.current_day_index(), 0);
    }

    #[test]
    fn test_detector_same_day_no_boundary() {
        let mut detector = DayBoundaryDetector::new(DayBoundaryConfig::us_equity());
        let ts1 = 1704067200_000_000_000i64;
        let ts2 = ts1 + 1 * NS_PER_HOUR; // 1 hour later

        detector.check_boundary(ts1);
        assert!(detector.check_boundary(ts2).is_none());
        assert_eq!(detector.current_day_index(), 0);
    }

    #[test]
    fn test_detector_gap_triggers_boundary() {
        let config = DayBoundaryConfig::us_equity().with_gap_threshold_hours(4);
        let mut detector = DayBoundaryDetector::new(config);

        let ts1 = 1704067200_000_000_000i64;
        let ts2 = ts1 + 5 * NS_PER_HOUR; // 5 hours later (exceeds 4h threshold)

        detector.check_boundary(ts1);
        let boundary = detector.check_boundary(ts2);

        assert!(boundary.is_some());
        let b = boundary.unwrap();
        assert_eq!(b.previous_day_index, 0);
        assert_eq!(b.new_day_index, 1);
        assert_eq!(b.gap_ns, 5 * NS_PER_HOUR);
        assert_eq!(detector.current_day_index(), 1);
    }

    #[test]
    fn test_detector_multiple_boundaries() {
        let config = DayBoundaryConfig::us_equity().with_gap_threshold_hours(4);
        let mut detector = DayBoundaryDetector::new(config);

        let ts1 = 1704067200_000_000_000i64;
        let ts2 = ts1 + 20 * NS_PER_HOUR; // Day 2
        let ts3 = ts2 + 20 * NS_PER_HOUR; // Day 3

        detector.check_boundary(ts1);
        assert!(detector.check_boundary(ts2).is_some());
        assert_eq!(detector.current_day_index(), 1);

        assert!(detector.check_boundary(ts3).is_some());
        assert_eq!(detector.current_day_index(), 2);
        assert_eq!(detector.boundaries_detected(), 2);
    }

    #[test]
    fn test_detector_fixed_boundary_midnight() {
        let config = DayBoundaryConfig::crypto(); // Uses fixed midnight boundary
        let mut detector = DayBoundaryDetector::new(config);

        // Day 1 timestamp (some time during the day)
        let ts1 = 1704067200_000_000_000i64; // Jan 1, 2024 00:00:00 UTC
        let ts2 = ts1 + 23 * NS_PER_HOUR; // 23:00 same day
        let ts3 = ts1 + 25 * NS_PER_HOUR; // 01:00 next day

        detector.check_boundary(ts1);
        assert!(detector.check_boundary(ts2).is_none()); // Same day
        assert!(detector.check_boundary(ts3).is_some()); // Crosses midnight
    }

    #[test]
    fn test_day_stats_recording() {
        let mut stats = DayBoundaryStats::new();

        stats.record_message(Some(1000), false, 100);
        stats.record_message(Some(2000), true, 50);
        stats.record_message(Some(3000), true, 75);

        assert_eq!(stats.messages, 3);
        assert_eq!(stats.trades, 2);
        assert_eq!(stats.total_volume, 125);
        assert_eq!(stats.first_ts, Some(1000));
        assert_eq!(stats.last_ts, Some(3000));
        assert_eq!(stats.duration_ns(), Some(2000));
    }

    #[test]
    fn test_day_boundary_info() {
        let boundary = DayBoundary {
            previous_day_end_ts: 1000,
            new_day_start_ts: 2000,
            gap_ns: 10 * NS_PER_HOUR,
            previous_day_index: 0,
            new_day_index: 1,
            previous_day_stats: DayBoundaryStats::new(),
        };

        assert_eq!(boundary.gap_hours(), 10.0);
        assert_eq!(boundary.gap_seconds(), 36000.0);
    }

    #[test]
    fn test_detector_reset() {
        let mut detector = DayBoundaryDetector::new(DayBoundaryConfig::us_equity());

        let ts1 = 1704067200_000_000_000i64;
        let ts2 = ts1 + 20 * NS_PER_HOUR;

        detector.check_boundary(ts1);
        detector.check_boundary(ts2);
        assert_eq!(detector.current_day_index(), 1);

        detector.reset();
        assert_eq!(detector.current_day_index(), 0);
        assert_eq!(detector.boundaries_detected(), 0);
        assert!(detector.last_timestamp().is_none());
    }

    #[test]
    fn test_config_builder_pattern() {
        let config = DayBoundaryConfig::us_equity()
            .with_gap_threshold_hours(6)
            .with_gap_detection();

        assert_eq!(config.gap_threshold_ns, 6 * NS_PER_HOUR);
        assert!(config.use_gap_detection);

        let config2 = DayBoundaryConfig::us_equity().with_fixed_boundaries();
        assert!(!config2.use_gap_detection);
    }
}

