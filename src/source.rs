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
//! - **Metadata Support**: Access to source information (symbol, date, path)
//! - **Testable**: Easy to mock for unit tests
//!
//! # Example
//!
//! ```ignore
//! use mbo_lob_reconstructor::source::{MarketDataSource, DbnSource};
//!
//! // Use with Databento files
//! let source = DbnSource::new("data/NVDA.mbo.dbn.zst")?;
//!
//! for msg in source.messages()? {
//!     // Process message...
//! }
//!
//! // Or with hot store
//! let source = DbnSource::with_hot_store("data/raw/NVDA.mbo.dbn.zst", &hot_store)?;
//! ```
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

use std::path::{Path, PathBuf};

use crate::error::Result;
use crate::types::MboMessage;

#[cfg(feature = "databento")]
use crate::hotstore::HotStoreManager;
#[cfg(feature = "databento")]
use crate::loader::{DbnLoader, MessageIterator};
#[cfg(feature = "databento")]
use std::fs::File;
#[cfg(feature = "databento")]
use std::io::BufReader;

// ============================================================================
// Source Metadata
// ============================================================================

/// Metadata about a market data source.
///
/// Provides information about the data being processed, useful for
/// logging, validation, and organizing output files.
#[derive(Debug, Clone, Default)]
pub struct SourceMetadata {
    /// Trading symbol (e.g., "NVDA", "AAPL")
    pub symbol: Option<String>,

    /// Trading date in YYYY-MM-DD format
    pub date: Option<String>,

    /// Original file path (if loaded from file)
    pub file_path: Option<PathBuf>,

    /// Data provider name (e.g., "databento", "custom")
    pub provider: Option<String>,

    /// Estimated message count (for progress tracking)
    pub estimated_messages: Option<u64>,

    /// File size in bytes (if applicable)
    pub file_size: Option<u64>,
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

    /// Set the file path.
    pub fn with_file_path(mut self, path: impl AsRef<Path>) -> Self {
        self.file_path = Some(path.as_ref().to_path_buf());
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

    /// Set the file size.
    pub fn with_file_size(mut self, size: u64) -> Self {
        self.file_size = Some(size);
        self
    }

    /// Extract metadata from a file path.
    ///
    /// Attempts to parse symbol and date from common filename patterns:
    /// - `NVDA_2025-02-03.mbo.dbn.zst` → symbol="NVDA", date="2025-02-03"
    /// - `NVDA.mbo.dbn.zst` → symbol="NVDA"
    pub fn from_path(path: impl AsRef<Path>) -> Self {
        let path = path.as_ref();
        let mut metadata = Self::new().with_file_path(path);

        // Get file size
        if let Ok(meta) = std::fs::metadata(path) {
            metadata.file_size = Some(meta.len());
        }

        // Parse filename for symbol and date
        if let Some(filename) = path.file_name().and_then(|n| n.to_str()) {
            // Remove extensions
            let base = filename
                .trim_end_matches(".zst")
                .trim_end_matches(".dbn")
                .trim_end_matches(".mbo")
                .trim_end_matches(".mbp-10");

            // Try to extract symbol and date
            // Pattern: SYMBOL_YYYY-MM-DD or SYMBOL
            if let Some(underscore_pos) = base.find('_') {
                let symbol = &base[..underscore_pos];
                let rest = &base[underscore_pos + 1..];

                metadata.symbol = Some(symbol.to_string());

                // Check if rest looks like a date (YYYY-MM-DD)
                if rest.len() >= 10 && rest.chars().nth(4) == Some('-') {
                    metadata.date = Some(rest[..10].to_string());
                }
            } else {
                metadata.symbol = Some(base.to_string());
            }
        }

        metadata
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
// DBN Source (Databento)
// ============================================================================

/// Market data source for Databento DBN files.
///
/// Wraps `DbnLoader` with the `MarketDataSource` trait, providing:
/// - Automatic metadata extraction from filename
/// - Optional hot store integration for faster loading
/// - Consistent interface with other data sources
///
/// # Example
///
/// ```ignore
/// use mbo_lob_reconstructor::source::{DbnSource, MarketDataSource};
///
/// // Load from compressed file
/// let source = DbnSource::new("data/NVDA_2025-02-03.mbo.dbn.zst")?;
///
/// println!("Symbol: {:?}", source.metadata().symbol);
/// println!("Date: {:?}", source.metadata().date);
///
/// for msg in source.messages()? {
///     // Process messages...
/// }
/// ```
///
/// # Hot Store Integration
///
/// For faster loading with pre-decompressed files:
///
/// ```ignore
/// use mbo_lob_reconstructor::source::DbnSource;
/// use mbo_lob_reconstructor::hotstore::HotStoreManager;
///
/// let hot_store = HotStoreManager::for_dbn("data/hot/");
/// let source = DbnSource::with_hot_store("data/raw/NVDA.mbo.dbn.zst", &hot_store)?;
///
/// // Automatically uses decompressed file if available (~5x faster)
/// for msg in source.messages()? {
///     // ...
/// }
/// ```
#[cfg(feature = "databento")]
#[cfg_attr(docsrs, doc(cfg(feature = "databento")))]
pub struct DbnSource {
    loader: DbnLoader,
    metadata: SourceMetadata,
}

#[cfg(feature = "databento")]
impl DbnSource {
    /// Create a new DBN source from a file path.
    ///
    /// # Arguments
    ///
    /// * `path` - Path to the .dbn or .dbn.zst file
    ///
    /// # Returns
    ///
    /// * `Ok(DbnSource)` - Ready to iterate
    /// * `Err(...)` - File not found or invalid
    pub fn new<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path = path.as_ref();
        let loader = DbnLoader::new(path)?;
        let mut metadata = SourceMetadata::from_path(path);
        metadata.provider = Some("databento".to_string());

        Ok(Self { loader, metadata })
    }

    /// Create a new DBN source with hot store path resolution.
    ///
    /// If a decompressed version exists in the hot store, it will be used
    /// for significantly faster loading (~5x speedup).
    ///
    /// # Arguments
    ///
    /// * `path` - Path to the .dbn.zst file
    /// * `hot_store` - Hot store manager for path resolution
    ///
    /// # Returns
    ///
    /// * `Ok(DbnSource)` - Using resolved path (decompressed if available)
    /// * `Err(...)` - File not found in either location
    pub fn with_hot_store<P: AsRef<Path>>(path: P, hot_store: &HotStoreManager) -> Result<Self> {
        let original_path = path.as_ref();
        let loader = DbnLoader::with_hot_store(original_path, hot_store)?;

        // Metadata from original path (for consistent symbol/date extraction)
        let mut metadata = SourceMetadata::from_path(original_path);
        metadata.provider = Some("databento".to_string());

        // Update file_path and file_size to reflect resolved path
        let resolved_path = loader.path();
        if resolved_path != original_path {
            log::info!(
                "Using hot store: {} -> {}",
                original_path.display(),
                resolved_path.display()
            );
            metadata.file_path = Some(resolved_path.to_path_buf());
            if let Ok(meta) = std::fs::metadata(resolved_path) {
                metadata.file_size = Some(meta.len());
            }
        }

        Ok(Self { loader, metadata })
    }

    /// Enable skipping invalid messages.
    ///
    /// When enabled, messages that fail to decode will be logged and skipped.
    pub fn skip_invalid(mut self, skip: bool) -> Self {
        self.loader = self.loader.skip_invalid(skip);
        self
    }

    /// Get the resolved file path being used.
    pub fn path(&self) -> &Path {
        self.loader.path()
    }
}

/// Type alias for the DBN message iterator.
#[cfg(feature = "databento")]
type DbnMessageIterator = MessageIterator<
    dbn::decode::dbn::Decoder<zstd::stream::read::Decoder<'static, BufReader<File>>>,
>;

#[cfg(feature = "databento")]
impl MarketDataSource for DbnSource {
    type MessageIter = DbnMessageIterator;

    fn messages(self) -> Result<Self::MessageIter> {
        self.loader.iter_messages()
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
    fn test_source_metadata_from_path() {
        // Full pattern: SYMBOL_DATE.mbo.dbn.zst
        let meta = SourceMetadata::from_path("/data/NVDA_2025-02-03.mbo.dbn.zst");
        assert_eq!(meta.symbol, Some("NVDA".to_string()));
        assert_eq!(meta.date, Some("2025-02-03".to_string()));

        // Symbol only
        let meta = SourceMetadata::from_path("/data/AAPL.mbo.dbn.zst");
        assert_eq!(meta.symbol, Some("AAPL".to_string()));
        assert!(meta.date.is_none());

        // Complex symbol
        let meta = SourceMetadata::from_path("/data/ES_2025-03-15.mbo.dbn.zst");
        assert_eq!(meta.symbol, Some("ES".to_string()));
        assert_eq!(meta.date, Some("2025-03-15".to_string()));
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
        let messages = vec![MboMessage::new(1, Action::Add, Side::Bid, 100_000_000_000, 100)];

        let source = VecSource::new(messages)
            .with_metadata(SourceMetadata::new().with_symbol("TEST").with_date("2025-01-01"));

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

    // ========================================================================
    // DbnSource tests (require databento feature)
    // ========================================================================

    #[cfg(feature = "databento")]
    mod dbn_tests {
        use super::*;

        #[test]
        fn test_dbn_source_nonexistent_file() {
            let result = DbnSource::new("/nonexistent/file.dbn.zst");
            assert!(result.is_err());
        }

        #[test]
        fn test_dbn_source_metadata_extraction() {
            // Test metadata extraction from path (doesn't require file to exist)
            let meta = SourceMetadata::from_path("/data/NVDA_2025-02-03.mbo.dbn.zst");
            assert_eq!(meta.symbol, Some("NVDA".to_string()));
            assert_eq!(meta.date, Some("2025-02-03".to_string()));
        }

        #[test]
        fn test_dbn_source_with_real_file() {
            // Only run if test file exists
            let test_file = std::env::var("TEST_DBN_FILE")
                .unwrap_or_else(|_| {
                    // Try common test file locations
                    let candidates = [
                        "../data/NVDA_2025-02-01_to_2025-09-30/NVDA_2025-02-03.mbo.dbn.zst",
                        "../../data/NVDA_2025-02-01_to_2025-09-30/NVDA_2025-02-03.mbo.dbn.zst",
                    ];
                    for path in candidates {
                        if std::path::Path::new(path).exists() {
                            return path.to_string();
                        }
                    }
                    String::new()
                });

            if test_file.is_empty() || !std::path::Path::new(&test_file).exists() {
                eprintln!("Skipping test_dbn_source_with_real_file: no test file available");
                return;
            }

            let source = DbnSource::new(&test_file).expect("Failed to create source");

            assert_eq!(source.metadata().provider, Some("databento".to_string()));
            assert!(source.metadata().symbol.is_some());

            // Count a few messages
            let mut count = 0;
            for _msg in source.messages().expect("Failed to get messages") {
                count += 1;
                if count >= 100 {
                    break;
                }
            }

            assert!(count > 0, "Should have read at least one message");
        }
    }
}

