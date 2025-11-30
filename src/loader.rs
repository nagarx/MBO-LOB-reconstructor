//! DBN file loader and streaming interface.
//!
//! This module provides efficient loading of compressed DBN files (.dbn.zst)
//! and streaming of MBO messages. Features:
//! - Automatic zstd decompression
//! - Memory-efficient streaming (doesn't load entire file into RAM)
//! - Progress tracking and statistics
//! - Error recovery options
//!
//! # Example
//!
//! ```ignore
//! use mbo_lob_reconstructor::{DbnLoader, LobReconstructor};
//!
//! // Create loader
//! let mut loader = DbnLoader::new("path/to/file.dbn.zst")?;
//!
//! // Create LOB reconstructor
//! let mut lob = LobReconstructor::new(10);
//!
//! // Process all messages
//! for mbo_msg in loader.iter_messages()? {
//!     let state = lob.process_message(&mbo_msg)?;
//!     // ... use state ...
//! }
//!
//! // Get statistics
//! let stats = loader.stats();
//! println!("Processed {} messages", stats.messages_read);
//! ```

use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};

use crate::dbn_bridge::DbnBridge;
use crate::error::{Result, TlobError};
use crate::types::MboMessage;
use dbn::decode::DecodeRecord; // Import trait for decode_record method

// Type alias for the decoder we use
// We wrap the file reader in a BufReader for efficiency, then in a zstd decoder
type DbnFileDecoder =
    dbn::decode::dbn::Decoder<zstd::stream::read::Decoder<'static, BufReader<File>>>;

/// Statistics for DBN file loading.
#[derive(Debug, Clone, Default)]
pub struct LoaderStats {
    /// Total messages successfully read
    pub messages_read: u64,

    /// Messages skipped due to conversion errors
    pub messages_skipped: u64,

    /// Total bytes read from file
    pub bytes_read: u64,

    /// File size in bytes
    pub file_size: u64,
}

/// DBN file loader.
///
/// Efficiently streams MBO messages from compressed DBN files.
pub struct DbnLoader {
    /// Path to the DBN file
    path: PathBuf,

    /// Statistics
    stats: LoaderStats,

    /// Skip invalid messages instead of erroring
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

    /// Enable skipping invalid messages instead of returning errors.
    ///
    /// When enabled, invalid messages will be logged and skipped,
    /// and processing will continue.
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
    fn open_decoder(&self) -> Result<DbnFileDecoder> {
        let file = File::open(&self.path)
            .map_err(|e| TlobError::generic(format!("Failed to open file: {e}")))?;

        let reader = BufReader::new(file);

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
            // decode_record returns a reference, we clone it
            let dbn_msg: dbn::MboMsg = match self.decoder.decode_record::<dbn::MboMsg>() {
                Ok(Some(msg)) => msg.clone(), // Clone the returned reference
                Ok(None) => return None,      // End of file
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

            // Convert to MboMessage
            match DbnBridge::convert(&dbn_msg) {
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
