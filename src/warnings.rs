//! Warning and issue tracking for LOB reconstruction.
//!
//! This module provides infrastructure for tracking warnings and issues
//! that occur during preprocessing. Warnings are categorized, timestamped,
//! and can be exported for debugging and root cause analysis.
//!
//! # Design Philosophy
//!
//! In production environments dealing with financial data, we need to:
//! 1. **Not fail silently**: Track all anomalies for later analysis
//! 2. **Not fail loudly**: Don't crash on recoverable issues
//! 3. **Enable debugging**: Provide enough context to find root causes
//! 4. **Support live environments**: Efficient, non-blocking logging
//!
//! # Example
//!
//! ```ignore
//! use mbo_lob_reconstructor::warnings::{WarningTracker, WarningCategory};
//!
//! let mut tracker = WarningTracker::new();
//!
//! // Track a warning
//! tracker.record(
//!     WarningCategory::InconsistentState,
//!     "Order 12345 not found at price level 100.00",
//!     Some(1234567890_000_000_000),
//!     Some(12345),
//! );
//!
//! // Export to file
//! tracker.export_to_file("warnings.json")?;
//!
//! // Get summary
//! let summary = tracker.summary();
//! println!("Total warnings: {}", summary.total);
//! ```

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

/// Category of warning for classification and filtering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum WarningCategory {
    /// Order not found when expected (cancel/modify/trade for unknown order)
    OrderNotFound,

    /// Price level not found when expected
    PriceLevelNotFound,

    /// Order not at expected price level
    OrderPriceMismatch,

    /// Book state is inconsistent (crossed/locked quotes)
    InconsistentState,

    /// Data quality issue (invalid price, size, etc.)
    DataQuality,

    /// Timestamp anomaly (out of order, gap, etc.)
    TimestampAnomaly,

    /// Message validation failure
    ValidationFailure,

    /// Book was cleared/reset
    BookCleared,

    /// Unknown or unexpected action
    UnknownAction,

    /// Performance warning (slow processing, memory, etc.)
    Performance,

    /// Other/uncategorized warning
    Other,
}

impl WarningCategory {
    /// Get a human-readable name for the category.
    pub fn name(&self) -> &'static str {
        match self {
            WarningCategory::OrderNotFound => "ORDER_NOT_FOUND",
            WarningCategory::PriceLevelNotFound => "PRICE_LEVEL_NOT_FOUND",
            WarningCategory::OrderPriceMismatch => "ORDER_PRICE_MISMATCH",
            WarningCategory::InconsistentState => "INCONSISTENT_STATE",
            WarningCategory::DataQuality => "DATA_QUALITY",
            WarningCategory::TimestampAnomaly => "TIMESTAMP_ANOMALY",
            WarningCategory::ValidationFailure => "VALIDATION_FAILURE",
            WarningCategory::BookCleared => "BOOK_CLEARED",
            WarningCategory::UnknownAction => "UNKNOWN_ACTION",
            WarningCategory::Performance => "PERFORMANCE",
            WarningCategory::Other => "OTHER",
        }
    }

    /// Get severity level (1=low, 2=medium, 3=high).
    pub fn severity(&self) -> u8 {
        match self {
            WarningCategory::OrderNotFound => 1,
            WarningCategory::PriceLevelNotFound => 2,
            WarningCategory::OrderPriceMismatch => 2,
            WarningCategory::InconsistentState => 3,
            WarningCategory::DataQuality => 2,
            WarningCategory::TimestampAnomaly => 2,
            WarningCategory::ValidationFailure => 2,
            WarningCategory::BookCleared => 1,
            WarningCategory::UnknownAction => 2,
            WarningCategory::Performance => 1,
            WarningCategory::Other => 1,
        }
    }
}

/// A single warning record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Warning {
    /// Unique warning ID (auto-incremented)
    pub id: u64,

    /// Warning category
    pub category: WarningCategory,

    /// Human-readable message
    pub message: String,

    /// Timestamp when the warning occurred (nanoseconds since epoch)
    /// This is the data timestamp, not the wall clock time
    pub data_timestamp: Option<i64>,

    /// Wall clock time when warning was recorded (nanoseconds since epoch)
    pub recorded_at: u64,

    /// Related order ID (if applicable)
    pub order_id: Option<u64>,

    /// Related price (if applicable)
    pub price: Option<i64>,

    /// Related size (if applicable)
    pub size: Option<u32>,

    /// Message sequence number (if applicable)
    pub sequence: Option<u64>,

    /// Additional context as key-value pairs
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub context: HashMap<String, String>,
}

impl Warning {
    /// Create a new warning with minimal information.
    pub fn new(id: u64, category: WarningCategory, message: impl Into<String>) -> Self {
        Self {
            id,
            category,
            message: message.into(),
            data_timestamp: None,
            recorded_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos() as u64)
                .unwrap_or(0),
            order_id: None,
            price: None,
            size: None,
            sequence: None,
            context: HashMap::new(),
        }
    }

    /// Set the data timestamp.
    pub fn with_data_timestamp(mut self, ts: i64) -> Self {
        self.data_timestamp = Some(ts);
        self
    }

    /// Set the order ID.
    pub fn with_order_id(mut self, order_id: u64) -> Self {
        self.order_id = Some(order_id);
        self
    }

    /// Set the price.
    pub fn with_price(mut self, price: i64) -> Self {
        self.price = Some(price);
        self
    }

    /// Set the size.
    pub fn with_size(mut self, size: u32) -> Self {
        self.size = Some(size);
        self
    }

    /// Set the sequence number.
    pub fn with_sequence(mut self, sequence: u64) -> Self {
        self.sequence = Some(sequence);
        self
    }

    /// Add context key-value pair.
    pub fn with_context(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.context.insert(key.into(), value.into());
        self
    }
}

/// Summary statistics for warnings.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WarningSummary {
    /// Total number of warnings
    pub total: u64,

    /// Count by category
    pub by_category: HashMap<String, u64>,

    /// Count by severity
    pub by_severity: HashMap<u8, u64>,

    /// First warning timestamp
    pub first_timestamp: Option<i64>,

    /// Last warning timestamp
    pub last_timestamp: Option<i64>,

    /// Number of unique order IDs involved
    pub unique_orders: u64,
}

/// Configuration for warning tracker.
#[derive(Debug, Clone)]
pub struct WarningTrackerConfig {
    /// Maximum number of warnings to keep in memory
    pub max_warnings: usize,

    /// Whether to log warnings to stderr
    pub log_to_stderr: bool,

    /// Minimum severity to log (1=all, 2=medium+, 3=high only)
    pub min_log_severity: u8,

    /// Whether to deduplicate similar warnings
    pub deduplicate: bool,

    /// Time window for deduplication (nanoseconds)
    pub dedupe_window_ns: u64,
}

impl Default for WarningTrackerConfig {
    fn default() -> Self {
        Self {
            max_warnings: 100_000,
            log_to_stderr: true,
            min_log_severity: 1,
            deduplicate: true,
            dedupe_window_ns: 1_000_000_000, // 1 second
        }
    }
}

/// Thread-safe warning tracker for preprocessing.
pub struct WarningTracker {
    /// Configuration
    config: WarningTrackerConfig,

    /// Stored warnings
    warnings: Vec<Warning>,

    /// Counter for unique IDs
    next_id: AtomicU64,

    /// Count by category (for fast summary)
    category_counts: HashMap<WarningCategory, u64>,

    /// Recent warnings for deduplication (category -> (message_hash, timestamp))
    recent: HashMap<WarningCategory, Vec<(u64, u64)>>,

    /// Unique order IDs seen in warnings
    unique_orders: std::collections::HashSet<u64>,
}

impl WarningTracker {
    /// Create a new warning tracker with default configuration.
    pub fn new() -> Self {
        Self::with_config(WarningTrackerConfig::default())
    }

    /// Create a new warning tracker with custom configuration.
    pub fn with_config(config: WarningTrackerConfig) -> Self {
        Self {
            config,
            warnings: Vec::new(),
            next_id: AtomicU64::new(1),
            category_counts: HashMap::new(),
            recent: HashMap::new(),
            unique_orders: std::collections::HashSet::new(),
        }
    }

    /// Record a warning.
    ///
    /// Returns the warning ID if recorded, or None if deduplicated.
    pub fn record(&mut self, warning: Warning) -> Option<u64> {
        // Check deduplication
        if self.config.deduplicate {
            let msg_hash = self.hash_message(&warning.message);
            let now = warning.recorded_at;

            if let Some(recent_list) = self.recent.get_mut(&warning.category) {
                // Clean old entries
                recent_list.retain(|(_, ts)| now - *ts < self.config.dedupe_window_ns);

                // Check if duplicate
                if recent_list.iter().any(|(h, _)| *h == msg_hash) {
                    return None;
                }

                recent_list.push((msg_hash, now));
            } else {
                self.recent.insert(warning.category, vec![(msg_hash, now)]);
            }
        }

        // Log to stderr if enabled
        if self.config.log_to_stderr && warning.category.severity() >= self.config.min_log_severity
        {
            eprintln!(
                "[WARNING] [{}] {}: {}",
                warning.category.name(),
                warning.id,
                warning.message
            );
        }

        // Track order ID
        if let Some(order_id) = warning.order_id {
            self.unique_orders.insert(order_id);
        }

        // Update category count
        *self.category_counts.entry(warning.category).or_insert(0) += 1;

        let id = warning.id;

        // Store warning (with capacity limit)
        if self.warnings.len() < self.config.max_warnings {
            self.warnings.push(warning);
        }

        Some(id)
    }

    /// Record a simple warning with just category and message.
    pub fn record_simple(
        &mut self,
        category: WarningCategory,
        message: impl Into<String>,
    ) -> Option<u64> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let warning = Warning::new(id, category, message);
        self.record(warning)
    }

    /// Record a warning with order context.
    pub fn record_order_warning(
        &mut self,
        category: WarningCategory,
        message: impl Into<String>,
        order_id: u64,
        price: Option<i64>,
        timestamp: Option<i64>,
    ) -> Option<u64> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let mut warning = Warning::new(id, category, message).with_order_id(order_id);

        if let Some(p) = price {
            warning = warning.with_price(p);
        }
        if let Some(ts) = timestamp {
            warning = warning.with_data_timestamp(ts);
        }

        self.record(warning)
    }

    /// Get the number of warnings recorded.
    pub fn len(&self) -> usize {
        self.warnings.len()
    }

    /// Check if no warnings have been recorded.
    pub fn is_empty(&self) -> bool {
        self.warnings.is_empty()
    }

    /// Get total count including deduplicated.
    pub fn total_count(&self) -> u64 {
        self.category_counts.values().sum()
    }

    /// Get count for a specific category.
    pub fn count_by_category(&self, category: WarningCategory) -> u64 {
        *self.category_counts.get(&category).unwrap_or(&0)
    }

    /// Get all warnings.
    pub fn warnings(&self) -> &[Warning] {
        &self.warnings
    }

    /// Get warnings by category.
    pub fn warnings_by_category(&self, category: WarningCategory) -> Vec<&Warning> {
        self.warnings
            .iter()
            .filter(|w| w.category == category)
            .collect()
    }

    /// Get summary statistics.
    pub fn summary(&self) -> WarningSummary {
        let mut by_category = HashMap::new();
        let mut by_severity = HashMap::new();

        for (cat, count) in &self.category_counts {
            by_category.insert(cat.name().to_string(), *count);
            *by_severity.entry(cat.severity()).or_insert(0) += *count;
        }

        let first_timestamp = self.warnings.first().and_then(|w| w.data_timestamp);
        let last_timestamp = self.warnings.last().and_then(|w| w.data_timestamp);

        WarningSummary {
            total: self.total_count(),
            by_category,
            by_severity,
            first_timestamp,
            last_timestamp,
            unique_orders: self.unique_orders.len() as u64,
        }
    }

    /// Export warnings to a JSON file.
    pub fn export_to_file(&self, path: impl AsRef<Path>) -> std::io::Result<()> {
        let file = File::create(path)?;
        let mut writer = BufWriter::new(file);

        // Write header
        writeln!(writer, "{{")?;

        // Write summary
        let summary = self.summary();
        writeln!(
            writer,
            "  \"summary\": {},",
            serde_json::to_string(&summary).unwrap_or_default()
        )?;

        // Write warnings array
        writeln!(writer, "  \"warnings\": [")?;

        for (i, warning) in self.warnings.iter().enumerate() {
            let json = serde_json::to_string(warning).unwrap_or_default();
            if i < self.warnings.len() - 1 {
                writeln!(writer, "    {json},")?;
            } else {
                writeln!(writer, "    {json}")?;
            }
        }

        writeln!(writer, "  ]")?;
        writeln!(writer, "}}")?;

        writer.flush()?;
        Ok(())
    }

    /// Export warnings to a CSV file (for spreadsheet analysis).
    pub fn export_to_csv(&self, path: impl AsRef<Path>) -> std::io::Result<()> {
        let file = File::create(path)?;
        let mut writer = BufWriter::new(file);

        // Header
        writeln!(
            writer,
            "id,category,severity,message,data_timestamp,recorded_at,order_id,price,size,sequence"
        )?;

        for warning in &self.warnings {
            writeln!(
                writer,
                "{},{},{},{:?},{},{},{},{},{},{}",
                warning.id,
                warning.category.name(),
                warning.category.severity(),
                warning.message,
                warning
                    .data_timestamp
                    .map(|t| t.to_string())
                    .unwrap_or_default(),
                warning.recorded_at,
                warning.order_id.map(|o| o.to_string()).unwrap_or_default(),
                warning.price.map(|p| p.to_string()).unwrap_or_default(),
                warning.size.map(|s| s.to_string()).unwrap_or_default(),
                warning.sequence.map(|s| s.to_string()).unwrap_or_default(),
            )?;
        }

        writer.flush()?;
        Ok(())
    }

    /// Clear all warnings.
    pub fn clear(&mut self) {
        self.warnings.clear();
        self.category_counts.clear();
        self.recent.clear();
        self.unique_orders.clear();
    }

    /// Simple hash for deduplication.
    fn hash_message(&self, message: &str) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        message.hash(&mut hasher);
        hasher.finish()
    }
}

impl Default for WarningTracker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_warning_category_names() {
        assert_eq!(WarningCategory::OrderNotFound.name(), "ORDER_NOT_FOUND");
        assert_eq!(
            WarningCategory::InconsistentState.name(),
            "INCONSISTENT_STATE"
        );
    }

    #[test]
    fn test_warning_category_severity() {
        assert_eq!(WarningCategory::OrderNotFound.severity(), 1);
        assert_eq!(WarningCategory::InconsistentState.severity(), 3);
    }

    #[test]
    fn test_warning_creation() {
        let warning = Warning::new(1, WarningCategory::OrderNotFound, "Test message")
            .with_order_id(12345)
            .with_price(100_000_000_000)
            .with_data_timestamp(1234567890_000_000_000);

        assert_eq!(warning.id, 1);
        assert_eq!(warning.category, WarningCategory::OrderNotFound);
        assert_eq!(warning.message, "Test message");
        assert_eq!(warning.order_id, Some(12345));
        assert_eq!(warning.price, Some(100_000_000_000));
        assert_eq!(warning.data_timestamp, Some(1234567890_000_000_000));
    }

    #[test]
    fn test_warning_tracker_basic() {
        let mut tracker = WarningTracker::new();

        tracker.record_simple(WarningCategory::OrderNotFound, "Order 123 not found");
        tracker.record_simple(WarningCategory::OrderNotFound, "Order 456 not found");
        tracker.record_simple(WarningCategory::InconsistentState, "Book crossed");

        assert_eq!(tracker.len(), 3);
        assert_eq!(tracker.count_by_category(WarningCategory::OrderNotFound), 2);
        assert_eq!(
            tracker.count_by_category(WarningCategory::InconsistentState),
            1
        );
    }

    #[test]
    fn test_warning_tracker_deduplication() {
        let mut config = WarningTrackerConfig::default();
        config.deduplicate = true;
        config.dedupe_window_ns = 1_000_000_000_000; // Very large window for test
        config.log_to_stderr = false;

        let mut tracker = WarningTracker::with_config(config);

        // Same message should be deduplicated
        let id1 = tracker.record_simple(WarningCategory::OrderNotFound, "Order 123 not found");
        let id2 = tracker.record_simple(WarningCategory::OrderNotFound, "Order 123 not found");

        assert!(id1.is_some());
        assert!(id2.is_none()); // Deduplicated
        assert_eq!(tracker.len(), 1);
    }

    #[test]
    fn test_warning_tracker_summary() {
        let mut config = WarningTrackerConfig::default();
        config.log_to_stderr = false;

        let mut tracker = WarningTracker::with_config(config);

        tracker.record_order_warning(
            WarningCategory::OrderNotFound,
            "Order not found",
            12345,
            Some(100_000_000_000),
            Some(1234567890_000_000_000),
        );

        tracker.record_order_warning(
            WarningCategory::OrderNotFound,
            "Another order not found",
            67890,
            None,
            None,
        );

        let summary = tracker.summary();
        assert_eq!(summary.total, 2);
        assert_eq!(summary.unique_orders, 2);
        assert_eq!(summary.by_category.get("ORDER_NOT_FOUND"), Some(&2));
    }

    #[test]
    fn test_warning_with_context() {
        let warning = Warning::new(1, WarningCategory::DataQuality, "Invalid price")
            .with_context("expected_range", "0-1000000")
            .with_context("actual_value", "-100");

        assert_eq!(
            warning.context.get("expected_range"),
            Some(&"0-1000000".to_string())
        );
        assert_eq!(
            warning.context.get("actual_value"),
            Some(&"-100".to_string())
        );
    }
}
