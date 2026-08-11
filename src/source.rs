//! Market data source abstraction for flexible data ingestion.
//!
//! This module provides a trait-based abstraction over market data sources,
//! enabling the pipeline to work with different data providers without
//! modification.
//!
//! # Design Goals
//!
//! - **Provider Agnostic**: Works with Databento, other vendors, or mock data
//! - **Iterator-Based**: Simple streaming interface
//! - **Metadata Support**: Access to caller-supplied symbol/date hints
//! - **Testable**: Easy to mock for unit tests
//!
//! `MarketDataSource` is intentionally limited to already-decoded in-memory or
//! synthetic messages. Physical DBN files require typed decode errors and
//! source-identity binding, so production file consumers use `StrictDbnLoaderV1`
//! directly rather than this infallible iterator trait.
//!
//! # Implementing Custom Sources
//!
//! ```ignore
//! use mbo_lob_reconstructor::source::{MarketDataSource, SourceMetadata};
//! use mbo_lob_reconstructor::{MboMessage, Result};
//!
//! struct MyDataSource {
//!     messages: Vec<MboMessage>,
//!     metadata: SourceMetadata,
//! }
//!
//! impl MarketDataSource for MyDataSource {
//!     type MessageIter = std::vec::IntoIter<MboMessage>;
//!
//!     fn messages(self) -> Result<Self::MessageIter> {
//!         Ok(self.messages.into_iter())
//!     }
//!
//!     fn metadata(&self) -> &SourceMetadata {
//!         &self.metadata
//!     }
//! }
//! ```

use crate::error::Result;
use crate::types::MboMessage;
use serde::{Deserialize, Serialize};

// ============================================================================
// Source Metadata
// ============================================================================

/// Metadata about a market data source.
///
/// Provides information about the data being processed, useful for
/// logging, validation, and organizing output files.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceMetadata {
    /// Trading symbol (e.g., "NVDA", "AAPL")
    pub symbol: Option<String>,

    /// Trading date in YYYY-MM-DD format
    pub date: Option<String>,

    /// Data provider name (e.g., "databento", "custom")
    pub provider: Option<String>,

    /// Estimated message count (for progress tracking)
    pub estimated_messages: Option<u64>,
}

impl SourceMetadata {
    /// Create new empty metadata.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the symbol.
    pub fn with_symbol(mut self, symbol: impl Into<String>) -> Self {
        self.symbol = Some(symbol.into());
        self
    }

    /// Set the date.
    pub fn with_date(mut self, date: impl Into<String>) -> Self {
        self.date = Some(date.into());
        self
    }

    /// Set the provider.
    pub fn with_provider(mut self, provider: impl Into<String>) -> Self {
        self.provider = Some(provider.into());
        self
    }

    /// Set the estimated message count.
    pub fn with_estimated_messages(mut self, count: u64) -> Self {
        self.estimated_messages = Some(count);
        self
    }
}

// ============================================================================
// Market Data Source Trait
// ============================================================================

/// Trait for market data sources.
///
/// This trait abstracts over different data sources, allowing the pipeline
/// to work with any provider that can yield MBO messages.
///
/// # Implementation Notes
///
/// - `messages()` consumes `self` to allow single-pass iteration
/// - The returned iterator should yield `MboMessage` directly
/// - Metadata should be populated before calling `messages()`
///
/// # Example Implementation
///
/// ```ignore
/// struct VecSource {
///     messages: Vec<MboMessage>,
///     metadata: SourceMetadata,
/// }
///
/// impl MarketDataSource for VecSource {
///     type MessageIter = std::vec::IntoIter<MboMessage>;
///
///     fn messages(self) -> Result<Self::MessageIter> {
///         Ok(self.messages.into_iter())
///     }
///
///     fn metadata(&self) -> &SourceMetadata {
///         &self.metadata
///     }
/// }
/// ```
pub trait MarketDataSource {
    /// The iterator type for messages.
    type MessageIter: Iterator<Item = MboMessage>;

    /// Consume the source and return an iterator over messages.
    ///
    /// # Returns
    ///
    /// * `Ok(Iterator)` - Iterator over MBO messages
    /// * `Err(...)` - Failed to open/read the source
    fn messages(self) -> Result<Self::MessageIter>;

    /// Get metadata about the source.
    ///
    /// Should return populated metadata including symbol, date, etc.
    fn metadata(&self) -> &SourceMetadata;
}

// ============================================================================
// Vector Source (for testing)
// ============================================================================

/// A simple in-memory source for testing.
///
/// Useful for unit tests and simulations.
///
/// # Example
///
/// ```
/// use mbo_lob_reconstructor::source::{VecSource, MarketDataSource, SourceMetadata};
/// use mbo_lob_reconstructor::{MboMessage, Action, Side};
///
/// let messages = vec![
///     MboMessage::new(1, Action::Add, Side::Bid, 100_000_000_000, 100),
///     MboMessage::new(2, Action::Add, Side::Ask, 100_010_000_000, 100),
/// ];
///
/// let source = VecSource::new(messages)
///     .with_metadata(SourceMetadata::new().with_symbol("TEST"));
///
/// let mut count = 0;
/// for _msg in source.messages().unwrap() {
///     count += 1;
/// }
/// assert_eq!(count, 2);
/// ```
pub struct VecSource {
    messages: Vec<MboMessage>,
    metadata: SourceMetadata,
}

impl VecSource {
    /// Create a new vector source.
    pub fn new(messages: Vec<MboMessage>) -> Self {
        Self {
            metadata: SourceMetadata::new()
                .with_provider("memory")
                .with_estimated_messages(messages.len() as u64),
            messages,
        }
    }

    /// Set custom metadata.
    pub fn with_metadata(mut self, metadata: SourceMetadata) -> Self {
        self.metadata = metadata;
        self
    }
}

impl MarketDataSource for VecSource {
    type MessageIter = std::vec::IntoIter<MboMessage>;

    fn messages(self) -> Result<Self::MessageIter> {
        Ok(self.messages.into_iter())
    }

    fn metadata(&self) -> &SourceMetadata {
        &self.metadata
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Action, Side};

    #[test]
    fn test_source_metadata_new() {
        let meta = SourceMetadata::new();
        assert!(meta.symbol.is_none());
        assert!(meta.date.is_none());
    }

    #[test]
    fn test_source_metadata_builder() {
        let meta = SourceMetadata::new()
            .with_symbol("NVDA")
            .with_date("2025-02-03")
            .with_provider("databento")
            .with_estimated_messages(1000);

        assert_eq!(meta.symbol, Some("NVDA".to_string()));
        assert_eq!(meta.date, Some("2025-02-03".to_string()));
        assert_eq!(meta.provider, Some("databento".to_string()));
        assert_eq!(meta.estimated_messages, Some(1000));
    }

    #[test]
    fn test_vec_source_basic() {
        let messages = vec![
            MboMessage::new(1, Action::Add, Side::Bid, 100_000_000_000, 100),
            MboMessage::new(2, Action::Add, Side::Ask, 100_010_000_000, 50),
        ];

        let source = VecSource::new(messages);

        assert_eq!(source.metadata().estimated_messages, Some(2));
        assert_eq!(source.metadata().provider, Some("memory".to_string()));

        let collected: Vec<_> = source.messages().unwrap().collect();
        assert_eq!(collected.len(), 2);
        assert_eq!(collected[0].order_id, 1);
        assert_eq!(collected[1].order_id, 2);
    }

    #[test]
    fn test_vec_source_with_metadata() {
        let messages = vec![MboMessage::new(
            1,
            Action::Add,
            Side::Bid,
            100_000_000_000,
            100,
        )];

        let source = VecSource::new(messages).with_metadata(
            SourceMetadata::new()
                .with_symbol("TEST")
                .with_date("2025-01-01"),
        );

        assert_eq!(source.metadata().symbol, Some("TEST".to_string()));
        assert_eq!(source.metadata().date, Some("2025-01-01".to_string()));
    }

    #[test]
    fn test_vec_source_empty() {
        let source = VecSource::new(Vec::new());

        assert_eq!(source.metadata().estimated_messages, Some(0));

        let collected: Vec<_> = source.messages().unwrap().collect();
        assert!(collected.is_empty());
    }
}
