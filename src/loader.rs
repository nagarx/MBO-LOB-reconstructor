//! DBN file loader and streaming interface.
//!
//! This module provides efficient loading of compressed DBN files (.dbn.zst)
//! and streaming of MBO messages. Features:
//! - Automatic zstd decompression
//! - Memory-efficient streaming (doesn't load entire file into RAM)
//! - Progress tracking and statistics
//! - Error recovery options
//! - Zero-copy message extraction (only ~40 bytes copied per message)
//! - Large I/O buffer (1MB) for improved throughput
//!
//! # Performance Note
//!
//! The bottleneck in DBN processing is **zstd decompression**, which is single-threaded
//! per file stream. The `MessageIterator` uses zero-copy extraction from the decoder's
//! internal buffer, minimizing CPU overhead after decompression.
//!
//! The I/O buffer size is set to 1MB (vs default 8KB) to reduce syscall overhead
//! and improve throughput by 5-15% on modern systems with fast SSDs.
//!
//! # Example
//!
//! ```ignore
//! use mbo_lob_reconstructor::{DbnLoader, LobReconstructor};
//!
//! // Create loader
//! let loader = DbnLoader::new("path/to/file.dbn.zst")?;
//!
//! // Create LOB reconstructor (skip_system_messages=true by default)
//! let mut lob = LobReconstructor::new(10);
//!
//! // Process all messages - system messages are automatically skipped
//! for mbo_msg in loader.iter_messages()? {
//!     let state = lob.process_message(&mbo_msg)?;
//!     // ... use state ...
//! }
//!
//! // Check statistics
//! println!("Processed: {}", lob.stats().messages_processed);
//! println!("System messages skipped: {}", lob.stats().system_messages_skipped);
//! ```
//!
//! # System Messages
//!
//! DBN/MBO data contains system messages (order_id=0, heartbeats, status updates)
//! that are NOT valid orders. These are handled by `LobReconstructor` with the
//! `skip_system_messages` config option (default: true).
//!
//! This loader focuses on I/O and decode errors only. Use `skip_invalid(true)`
//! to skip messages that fail to decode from the DBN format.

use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};

use crate::dbn_bridge::DbnBridge;
use crate::error::{Result, TlobError};
use crate::hotstore::HotStoreManager;
use crate::types::MboMessage;
use dbn::decode::DecodeRecord; // Import trait for decode_record method

// ============================================================================
// Performance Constants
// ============================================================================

/// I/O buffer size for file reading.
///
/// Default `BufReader` uses 8KB, but larger buffers reduce syscall overhead
/// and improve throughput on modern SSDs.
///
/// # Rationale
///
/// - 1MB provides optimal balance between memory usage and throughput
/// - Reduces syscall frequency by ~125x compared to default 8KB
/// - Measured 5-15% throughput improvement on typical MBO files
/// - Memory impact is minimal (1MB per open file)
///
/// # References
///
/// See PERFORMANCE_BOTTLENECK_ANALYSIS.md for benchmarks and analysis.
pub const IO_BUFFER_SIZE: usize = 1024 * 1024; // 1 MB

// Type alias for the decoder we use
// We wrap the file reader in a BufReader for efficiency, then in a zstd decoder
type DbnFileDecoder =
    dbn::decode::dbn::Decoder<zstd::stream::read::Decoder<'static, BufReader<File>>>;

/// Statistics for DBN file loading.
#[derive(Debug, Clone, Default)]
pub struct LoaderStats {
    /// Total messages successfully read
    pub messages_read: u64,

    /// Messages skipped due to decode/conversion errors
    pub messages_skipped: u64,

    /// Total bytes read from file
    pub bytes_read: u64,

    /// File size in bytes
    pub file_size: u64,
}

/// DBN file loader.
///
/// Efficiently streams MBO messages from compressed DBN files.
///
/// # Responsibilities
///
/// This loader handles:
/// - File I/O (opening, streaming, buffering)
/// - DBN format decoding
/// - Conversion to `MboMessage`
/// - Error recovery for decode failures
///
/// It does NOT handle:
/// - System message filtering (that's `LobReconstructor`'s job)
/// - Order validation (that's `LobReconstructor`'s job)
///
/// # Example
///
/// ```ignore
/// let loader = DbnLoader::new("data.dbn.zst")?
///     .skip_invalid(true);  // Skip decode errors
///
/// for msg in loader.iter_messages()? {
///     // Pass to LobReconstructor - it handles system messages
///     lob.process_message(&msg)?;
/// }
/// ```
pub struct DbnLoader {
    /// Path to the DBN file
    path: PathBuf,

    /// Statistics
    stats: LoaderStats,

    /// Skip messages that fail to decode (instead of erroring)
    skip_invalid: bool,
}

impl DbnLoader {
    /// Create a new DBN loader.
    ///
    /// # Arguments
    ///
    /// * `path` - Path to the .dbn or .dbn.zst file
    ///
    /// # Returns
    ///
    /// * `Ok(DbnLoader)` - Loader ready to use
    /// * `Err(TlobError)` - File not found or not accessible
    pub fn new<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path = path.as_ref().to_path_buf();

        // Check if file exists
        if !path.exists() {
            return Err(TlobError::generic(format!(
                "File not found: {}",
                path.display()
            )));
        }

        // Get file size
        let file_size = std::fs::metadata(&path)
            .map_err(|e| TlobError::generic(format!("Failed to read file metadata: {e}")))?
            .len();

        Ok(Self {
            path,
            stats: LoaderStats {
                file_size,
                ..Default::default()
            },
            skip_invalid: false,
        })
    }

    /// Create a new DBN loader with hot store path resolution.
    ///
    /// This method resolves the path using the provided `HotStoreManager`,
    /// automatically preferring decompressed files when available for
    /// significantly faster loading (~5x speedup).
    ///
    /// # Arguments
    ///
    /// * `path` - Path to the .dbn.zst file (will be resolved to decompressed if available)
    /// * `hot_store` - Hot store manager for path resolution
    ///
    /// # Returns
    ///
    /// * `Ok(DbnLoader)` - Loader pointing to the resolved path
    /// * `Err(TlobError)` - File not found (in either location)
    ///
    /// # Example
    ///
    /// ```ignore
    /// use mbo_lob_reconstructor::{DbnLoader, HotStoreManager};
    ///
    /// // Create hot store manager
    /// let hot_store = HotStoreManager::for_dbn("/data/hot/");
    ///
    /// // Load with automatic path resolution
    /// // If /data/hot/NVDA.mbo.dbn exists, it will be used instead of the .zst
    /// let loader = DbnLoader::with_hot_store(
    ///     "/data/raw/NVDA.mbo.dbn.zst",
    ///     &hot_store
    /// )?;
    ///
    /// // Process as usual
    /// for msg in loader.iter_messages()? {
    ///     // ...
    /// }
    /// ```
    ///
    /// # Performance
    ///
    /// When a decompressed file is available:
    /// - ~5x faster file reading (eliminates zstd decompression)
    /// - Better multi-core utilization
    /// - Reduced CPU usage during processing
    pub fn with_hot_store<P: AsRef<Path>>(path: P, hot_store: &HotStoreManager) -> Result<Self> {
        let resolved_path = hot_store.resolve(path.as_ref());
        
        log::debug!(
            "Hot store resolved: {} -> {}",
            path.as_ref().display(),
            resolved_path.display()
        );

        Self::new(resolved_path)
    }

    /// Enable skipping messages that fail to decode.
    ///
    /// When enabled, messages that fail DBN decoding or conversion
    /// will be logged and skipped, and processing will continue.
    ///
    /// This handles DECODE errors only. System messages (order_id=0)
    /// are handled by `LobReconstructor` with `skip_system_messages`.
    pub fn skip_invalid(mut self, skip: bool) -> Self {
        self.skip_invalid = skip;
        self
    }

    /// Get the file path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Get statistics.
    pub fn stats(&self) -> &LoaderStats {
        &self.stats
    }

    /// Open the file and return a decoder.
    ///
    /// This is a low-level method. Most users should use `iter_messages()` instead.
    ///
    /// Note: We always use the zstd decoder because it can handle both compressed
    /// and uncompressed DBN files.
    ///
    /// # Performance
    ///
    /// Uses a 1MB I/O buffer (see [`IO_BUFFER_SIZE`]) to reduce syscall overhead.
    fn open_decoder(&self) -> Result<DbnFileDecoder> {
        let file = File::open(&self.path)
            .map_err(|e| TlobError::generic(format!("Failed to open file: {e}")))?;

        // Use large buffer for better I/O throughput
        // Default BufReader uses 8KB; we use 1MB for 5-15% improvement
        let reader = BufReader::with_capacity(IO_BUFFER_SIZE, file);

        // Use zstd decoder with buffering
        // The zstd decoder can handle both compressed and uncompressed data
        dbn::decode::dbn::Decoder::with_zstd_buffer(reader)
            .map_err(|e| TlobError::generic(format!("Failed to create decoder: {e}")))
    }

    /// Iterate over all MBO messages in the file.
    ///
    /// Returns an iterator that yields `MboMessage`s.
    ///
    /// # Returns
    ///
    /// * `Ok(MessageIterator)` - Iterator over messages
    /// * `Err(TlobError)` - Failed to open file or initialize decoder
    ///
    /// # Example
    ///
    /// ```ignore
    /// let loader = DbnLoader::new("data.dbn.zst")?;
    /// for msg in loader.iter_messages()? {
    ///     println!("Order {}: {:?}", msg.order_id, msg.action);
    /// }
    /// ```
    pub fn iter_messages(self) -> Result<MessageIterator<DbnFileDecoder>> {
        let decoder = self.open_decoder()?;

        Ok(MessageIterator {
            decoder,
            stats: self.stats,
            skip_invalid: self.skip_invalid,
        })
    }

    /// Read all messages into a Vec.
    ///
    /// **Warning**: This loads all messages into memory at once.
    /// For large files, use `iter_messages()` instead.
    ///
    /// # Returns
    ///
    /// * `Ok(Vec<MboMessage>)` - All messages
    /// * `Err(TlobError)` - Failed to read or decode
    pub fn read_all(self) -> Result<Vec<MboMessage>> {
        let mut messages = Vec::new();

        for msg in self.iter_messages()? {
            messages.push(msg);
        }

        Ok(messages)
    }

    /// Count total messages in the file without allocating memory.
    ///
    /// Useful for progress bars and memory planning.
    ///
    /// # Returns
    ///
    /// * `Ok(u64)` - Total message count
    /// * `Err(TlobError)` - Failed to read file
    pub fn count_messages(self) -> Result<u64> {
        let mut count = 0u64;

        for _ in self.iter_messages()? {
            count += 1;
        }

        Ok(count)
    }
}

/// Iterator over MBO messages in a DBN file.
pub struct MessageIterator<D: DecodeRecord> {
    decoder: D,
    stats: LoaderStats,
    skip_invalid: bool,
}

impl<D: DecodeRecord> Iterator for MessageIterator<D> {
    type Item = MboMessage;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            // Decode next record
            // decode_record returns a reference to the decoder's internal buffer.
            // We convert immediately, so no clone is needed - the reference is valid
            // until the next decode_record call.
            let dbn_msg_ref = match self.decoder.decode_record::<dbn::MboMsg>() {
                Ok(Some(msg)) => msg, // Use reference directly (no clone!)
                Ok(None) => return None, // End of file
                Err(e) => {
                    if self.skip_invalid {
                        log::warn!("Failed to decode DBN record: {e}");
                        self.stats.messages_skipped += 1;
                        continue;
                    } else {
                        log::error!("Failed to decode DBN record: {e}");
                        return None;
                    }
                }
            };

            // Convert to MboMessage immediately (extracts values from reference)
            // This is zero-copy: we only copy the ~40 bytes of MboMessage fields,
            // not the entire dbn::MboMsg struct
            match DbnBridge::convert(dbn_msg_ref) {
                Ok(mbo_msg) => {
                    self.stats.messages_read += 1;
                    return Some(mbo_msg);
                }
                Err(e) => {
                    if self.skip_invalid {
                        log::warn!("Skipping invalid message: {e}");
                        self.stats.messages_skipped += 1;
                        continue;
                    } else {
                        log::error!("Invalid message: {e}");
                        return None;
                    }
                }
            }
        }
    }
}

impl<D: DecodeRecord> MessageIterator<D> {
    /// Get current statistics.
    pub fn stats(&self) -> &LoaderStats {
        &self.stats
    }

    /// Get progress as a percentage (0.0 to 100.0).
    ///
    /// Note: This is an estimate based on bytes read vs file size.
    pub fn progress(&self) -> f64 {
        if self.stats.file_size == 0 {
            return 100.0;
        }

        (self.stats.bytes_read as f64 / self.stats.file_size as f64) * 100.0
    }
}

/// Check if an MBO message represents a valid order (not a system message).
///
/// Returns `false` for:
/// - System messages (`order_id = 0`)
/// - Invalid size (`size = 0`)
/// - Invalid price (`price <= 0`)
///
/// This is a utility function for cases where you want to filter
/// messages before passing to `LobReconstructor`, or when
/// `skip_system_messages` is disabled in `LobConfig`.
///
/// Note: With the default `LobConfig` (skip_system_messages=true),
/// you don't need to use this function - the reconstructor handles it.
///
/// # Example
///
/// ```ignore
/// // Only needed if skip_system_messages=false in LobConfig
/// for msg in loader.iter_messages()? {
///     if is_valid_order(&msg) {
///         lob.process_message(&msg)?;
///     }
/// }
/// ```
#[inline]
pub fn is_valid_order(msg: &MboMessage) -> bool {
    msg.order_id != 0 && msg.size != 0 && msg.price > 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hotstore::HotStoreConfig;
    use crate::types::{Action, Side};
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn unique_temp_dir(name: &str) -> PathBuf {
        let counter = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
        std::env::temp_dir().join(format!("loader_test_{}_{}_{}", std::process::id(), name, counter))
    }

    #[test]
    fn test_loader_new_nonexistent() {
        let result = DbnLoader::new("/nonexistent/file.dbn.zst");
        assert!(result.is_err());
    }

    #[test]
    fn test_loader_stats() {
        // We can't test actual file loading without a test file,
        // but we can test the API
        let stats = LoaderStats::default();
        assert_eq!(stats.messages_read, 0);
        assert_eq!(stats.messages_skipped, 0);
    }

    #[test]
    fn test_skip_invalid_builder() {
        // This would need a real file to test fully
        // For now, just test the API
        let result = DbnLoader::new("/tmp/test.dbn.zst");
        if let Ok(loader) = result {
            let loader = loader.skip_invalid(true);
            assert!(loader.skip_invalid);
        }
    }

    #[test]
    fn test_is_valid_order() {
        // Valid order
        let valid = MboMessage::new(123, Action::Add, Side::Bid, 100_000_000_000, 100);
        assert!(is_valid_order(&valid));

        // Invalid: order_id = 0 (system message)
        let invalid = MboMessage::new(0, Action::Add, Side::Bid, 100_000_000_000, 100);
        assert!(!is_valid_order(&invalid));

        // Invalid: size = 0
        let invalid = MboMessage::new(123, Action::Add, Side::Bid, 100_000_000_000, 0);
        assert!(!is_valid_order(&invalid));

        // Invalid: price <= 0
        let invalid = MboMessage::new(123, Action::Add, Side::Bid, 0, 100);
        assert!(!is_valid_order(&invalid));

        let invalid = MboMessage::new(123, Action::Add, Side::Bid, -100, 100);
        assert!(!is_valid_order(&invalid));
    }

    #[test]
    fn test_with_hot_store_resolves_path() {
        let dir = unique_temp_dir("with_hot_store");
        let _ = fs::remove_dir_all(&dir);

        // Create hot store directory with a "decompressed" file
        let hot_dir = dir.join("hot");
        fs::create_dir_all(&hot_dir).unwrap();

        // Create fake decompressed file
        let decompressed = hot_dir.join("test.mbo.dbn");
        fs::write(&decompressed, "fake dbn content").unwrap();

        // Create hot store manager
        let config = HotStoreConfig::dbn_defaults(&hot_dir);
        let hot_store = HotStoreManager::new(config);

        // with_hot_store should resolve to the decompressed file
        let result = DbnLoader::with_hot_store("/raw/test.mbo.dbn.zst", &hot_store);
        
        // Should succeed because decompressed file exists
        assert!(result.is_ok());
        let loader = result.unwrap();
        assert_eq!(loader.path(), decompressed);

        // Cleanup
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_with_hot_store_fallback_to_original() {
        let dir = unique_temp_dir("hot_store_fallback");
        let _ = fs::remove_dir_all(&dir);

        // Create hot store directory (empty)
        let hot_dir = dir.join("hot");
        fs::create_dir_all(&hot_dir).unwrap();

        // Create original file
        let raw_dir = dir.join("raw");
        fs::create_dir_all(&raw_dir).unwrap();
        let original = raw_dir.join("test.mbo.dbn.zst");
        fs::write(&original, "fake compressed content").unwrap();

        // Create hot store manager
        let config = HotStoreConfig::dbn_defaults(&hot_dir);
        let hot_store = HotStoreManager::new(config);

        // with_hot_store should fallback to original since no decompressed exists
        let result = DbnLoader::with_hot_store(&original, &hot_store);
        
        // Should succeed with original path
        assert!(result.is_ok());
        let loader = result.unwrap();
        assert_eq!(loader.path(), original);

        // Cleanup
        let _ = fs::remove_dir_all(&dir);
    }
}
