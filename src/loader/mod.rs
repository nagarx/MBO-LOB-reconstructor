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

pub mod error;
pub use error::BoundaryError;

use std::fs::File;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use crate::dbn_bridge::DbnBridge;
use crate::error::{Result, TlobError};
use crate::hotstore::HotStoreManager;
use crate::types::MboMessage;
use dbn::decode::{DecodeRecord, DynDecoder}; // Import trait for decode_record method
use dbn::VersionUpgradePolicy;

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
// We wrap the file reader in a BufReader for efficiency, then in a CountingReader
// for byte-tracking observability (Phase M M.A.2 — closes F-008), then in a zstd
// decoder.
/// Decoder type that auto-detects compression (compressed or uncompressed DBN files).
type DbnFileDecoder = DynDecoder<'static, CountingReader<BufReader<File>>>;

/// Byte-counting wrapper around a `Read` source.
///
/// Tracks total bytes read from the underlying reader via an atomic counter
/// shared with the [`MessageIterator`] (Phase M M.A.2 — closes F-008 by
/// wiring `LoaderStats::bytes_read`).
///
/// # Decision 17 — Single-thread-per-iterator scope (Phase M REV 3)
///
/// The `Arc<AtomicU64>` counter is constructed inside `iter_messages()` and
/// `iter_messages_typed()` (M.A.3) and dropped when the iterator is dropped.
/// **It MUST NOT cross the `Rayon::par_iter` boundary in `BatchProcessor`** —
/// each parallel-day worker owns its own iterator + its own `Arc<AtomicU64>`,
/// so there is no inter-thread contention.
///
/// Cross-thread sharing of this counter would turn the un-contended
/// `lock xadd` (~25 ns) into 100s of ns under cache-line bouncing (Drepper,
/// "What Every Programmer Should Know About Memory" §6.3).
///
/// # Performance
///
/// `Ordering::Relaxed` `fetch_add` lowers to a single `lock xadd` on x86-64.
/// Since `BufReader` reads in 1MB chunks (per `IO_BUFFER_SIZE`), the
/// `fetch_add` fires ~once per ~17,000 messages — amortized cost is
/// effectively free (~0.001 ns/msg).
pub struct CountingReader<R: Read> {
    inner: R,
    bytes_read: Arc<AtomicU64>,
}

impl<R: Read> CountingReader<R> {
    /// Wrap a `Read` source with byte counting.
    ///
    /// Returns the wrapped reader. The caller retains the [`Arc<AtomicU64>`]
    /// handle (passed in) for read-side access via
    /// `bytes_read.load(Ordering::Relaxed)`.
    pub fn new(inner: R, bytes_read: Arc<AtomicU64>) -> Self {
        Self { inner, bytes_read }
    }
}

impl<R: Read> Read for CountingReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.bytes_read.fetch_add(n as u64, Ordering::Relaxed);
        Ok(n)
    }
}

impl<R: BufRead> BufRead for CountingReader<R> {
    fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
        // Buffer fill itself doesn't count as "consumed" bytes; only consume()
        // advances the byte counter. This matches BufRead semantics where
        // fill_buf() may expose more bytes than will be consumed (via consume(n)).
        self.inner.fill_buf()
    }

    fn consume(&mut self, amt: usize) {
        // Phase M M.A.2: track bytes actually consumed by the upstream
        // consumer. dbn::DynDecoder uses BufRead protocol (fill_buf + consume)
        // for zstd-compressed streams and Read protocol (read) for
        // uncompressed streams. The two code paths are disjoint per byte —
        // dbn calls EITHER read() OR fill_buf+consume() per chunk, never
        // both — so tracking in both `Read::read` AND `BufRead::consume`
        // gives exact byte counting without double-counting.
        self.bytes_read.fetch_add(amt as u64, Ordering::Relaxed);
        self.inner.consume(amt);
    }
}

/// Statistics for DBN file loading.
///
/// Phase M M.A.3 (REV 3): added `mid_record_eof` counter (Decision 5b —
/// observability-tier "stream did not end where expected"; surfaces via
/// [`LoaderStats::is_clean_eof()`] post-iteration).
#[derive(Debug, Clone, Default)]
pub struct LoaderStats {
    /// Total messages successfully read.
    pub messages_read: u64,

    /// Messages skipped due to decode/conversion errors (when `skip_invalid=true`).
    pub messages_skipped: u64,

    /// Total bytes consumed from file.
    ///
    /// Phase M M.A.2: now correctly populated via [`CountingReader`]
    /// (closes F-008 — pre-M.A.2 was always 0).
    pub bytes_read: u64,

    /// File size in bytes (populated at constructor from `metadata.len()`).
    pub file_size: u64,

    /// Number of mid-record EOFs observed during iteration (Phase M M.A.3
    /// Decision 5b).
    ///
    /// Increments when the underlying decoder returns
    /// `dbn::Error::Io(UnexpectedEof)` mid-stream — distinguished from clean
    /// EOF (where the iterator returns `None` cleanly).
    ///
    /// Surfaced via [`LoaderStats::is_clean_eof()`] for caller-decided
    /// abort/warn policy. The typed iterator returns `None` on both clean
    /// EOF and mid-record EOF; callers MUST check `is_clean_eof()` after
    /// iteration to detect torn streams.
    pub mid_record_eof: u64,
}

impl LoaderStats {
    /// Returns `true` if no mid-record EOF was observed during iteration.
    ///
    /// Phase M M.A.3 (REV 3 Decision 5b): caller-decides abort/warn discipline.
    ///
    /// # Usage
    ///
    /// ```ignore
    /// let mut iter = loader.iter_messages_typed()?;
    /// for msg_result in &mut iter {
    ///     let msg = msg_result?;
    ///     // ... process ...
    /// }
    /// let stats = iter.finalize();
    /// if !stats.is_clean_eof() {
    ///     // torn record detected; production callers should propagate Err,
    ///     // analytical callers should WARN + continue
    ///     log::warn!("Torn DBN record: mid_record_eof={}", stats.mid_record_eof);
    /// }
    /// ```
    #[inline]
    pub fn is_clean_eof(&self) -> bool {
        self.mid_record_eof == 0
    }
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
    fn open_decoder(&self) -> Result<(DbnFileDecoder, Arc<AtomicU64>)> {
        let file = File::open(&self.path)
            .map_err(|e| TlobError::generic(format!("Failed to open file: {e}")))?;

        // Use large buffer for better I/O throughput
        // Default BufReader uses 8KB; we use 1MB for 5-15% improvement
        let reader = BufReader::with_capacity(IO_BUFFER_SIZE, file);

        // Phase M M.A.2: wrap in CountingReader to track bytes_read for
        // F-008 closure. Arc shared between reader (writer side) and the
        // iterator (reader side via stats sync in next() loop).
        let bytes_read_handle = Arc::new(AtomicU64::new(0));
        let counting_reader = CountingReader::new(reader, Arc::clone(&bytes_read_handle));

        // Use DynDecoder to auto-detect compression
        // Handles both .dbn (uncompressed) and .dbn.zst (compressed) files
        let decoder = DynDecoder::inferred_with_buffer(counting_reader, VersionUpgradePolicy::AsIs)
            .map_err(|e| TlobError::generic(format!("Failed to create decoder: {e}")))?;

        Ok((decoder, bytes_read_handle))
    }

    /// Iterate over all MBO messages in the file (legacy untyped API).
    ///
    /// **DEPRECATED** since 0.2.0. Use [`Self::iter_messages_typed`] for the
    /// new typed iterator API that distinguishes Decode/Convert errors at
    /// compile time.
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
    #[cfg(feature = "legacy-iterator-api")]
    #[deprecated(
        since = "0.2.0",
        note = "Use iter_messages_typed instead (Item = Result<MboMessage, BoundaryError>). \
                This legacy API will be removed in 0.3.0 (calendar 2026-10-29) per \
                Phase M REV 3 Decision 2."
    )]
    pub fn iter_messages(self) -> Result<MessageIterator<DbnFileDecoder>> {
        let (decoder, bytes_read_handle) = self.open_decoder()?;

        Ok(MessageIterator {
            decoder,
            stats: self.stats,
            skip_invalid: self.skip_invalid,
            bytes_read_handle,
        })
    }

    /// Iterate over all MBO messages in the file (typed API, preferred).
    ///
    /// Phase M M.A.3 (REV 3 Decision 1): typed iterator yielding
    /// `Result<MboMessage, BoundaryError>` for compile-time error handling.
    ///
    /// # Returns
    ///
    /// * `Ok(TypedMessageIterator)` - Iterator yielding `Result<MboMessage, BoundaryError>`
    /// * `Err(TlobError)` - Failed to open file or initialize decoder
    ///
    /// # Mid-record EOF handling (Decision 5b)
    ///
    /// - Clean EOF → iterator returns `None`
    /// - Mid-record EOF (`UnexpectedEof` from decoder) → counter
    ///   `LoaderStats::mid_record_eof` increments AND iterator returns `None`
    /// - Decode/Convert errors → `Some(Err(BoundaryError::*))` propagates via `?`
    ///   (or, with `skip_invalid=true`, are logged + counted in `messages_skipped`)
    ///
    /// Callers MUST call [`TypedMessageIterator::finalize`] post-iteration
    /// (or check `iter.stats().is_clean_eof()`) to detect torn streams.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let loader = DbnLoader::new("data.dbn.zst")?;
    /// let mut iter = loader.iter_messages_typed()?;
    /// for msg_result in &mut iter {
    ///     let msg = msg_result?;  // BoundaryError::Decode/Convert propagates
    ///     // ... process msg ...
    /// }
    /// let stats = iter.finalize();
    /// if !stats.is_clean_eof() {
    ///     return Err(BoundaryError::Decode(format!(
    ///         "torn DBN: mid_record_eof={}", stats.mid_record_eof
    ///     )).into());
    /// }
    /// ```
    pub fn iter_messages_typed(self) -> Result<TypedMessageIterator<DbnFileDecoder>> {
        let (decoder, bytes_read_handle) = self.open_decoder()?;

        Ok(TypedMessageIterator {
            decoder,
            stats: self.stats,
            skip_invalid: self.skip_invalid,
            bytes_read_handle,
        })
    }

    /// Read all messages into a Vec (legacy untyped API).
    ///
    /// **DEPRECATED** — uses the legacy [`Self::iter_messages`] internally.
    /// Migrate to a manual loop over [`Self::iter_messages_typed`] for
    /// typed-error handling.
    ///
    /// **Warning**: This loads all messages into memory at once.
    /// For large files, use `iter_messages_typed()` directly instead.
    #[cfg(feature = "legacy-iterator-api")]
    #[deprecated(
        since = "0.2.0",
        note = "Use a manual loop over iter_messages_typed() for typed-error handling. \
                This convenience helper depends on legacy-iterator-api which is \
                scheduled for removal in 0.3.0 (calendar 2026-10-29)."
    )]
    pub fn read_all(self) -> Result<Vec<MboMessage>> {
        let mut messages = Vec::new();

        #[allow(deprecated)] // intentional internal use of legacy iter_messages
        for msg in self.iter_messages()? {
            messages.push(msg);
        }

        Ok(messages)
    }

    /// Count total messages in the file (legacy untyped API).
    ///
    /// **DEPRECATED** — uses the legacy [`Self::iter_messages`] internally.
    /// Migrate to a manual count over [`Self::iter_messages_typed`].
    #[cfg(feature = "legacy-iterator-api")]
    #[deprecated(
        since = "0.2.0",
        note = "Use a manual loop over iter_messages_typed() for typed-error handling. \
                This convenience helper depends on legacy-iterator-api which is \
                scheduled for removal in 0.3.0 (calendar 2026-10-29)."
    )]
    pub fn count_messages(self) -> Result<u64> {
        let mut count = 0u64;

        #[allow(deprecated)] // intentional internal use of legacy iter_messages
        for _ in self.iter_messages()? {
            count += 1;
        }

        Ok(count)
    }
}

/// Iterator over MBO messages in a DBN file (legacy untyped API).
///
/// **DEPRECATED** since 0.2.0. Use [`TypedMessageIterator`] for the new typed
/// iterator API with `Item = Result<MboMessage, BoundaryError>`.
///
/// Gated under the `legacy-iterator-api` feature flag (default-on for
/// transition; will be removed in 0.3.0 / calendar 2026-10-29 per Phase M
/// REV 3 Decision 2).
#[cfg(feature = "legacy-iterator-api")]
pub struct MessageIterator<D: DecodeRecord> {
    decoder: D,
    stats: LoaderStats,
    skip_invalid: bool,
    /// Phase M M.A.2: shared `Arc<AtomicU64>` with the inner [`CountingReader`].
    /// Updates `stats.bytes_read` on every `next()` call so `progress()` returns
    /// the correct fraction (closes F-008 — pre-M.A.2 `bytes_read` was always 0).
    /// See [`CountingReader`] for Decision 17 single-thread-per-iterator scope.
    bytes_read_handle: Arc<AtomicU64>,
}

#[cfg(feature = "legacy-iterator-api")]
impl<D: DecodeRecord> Iterator for MessageIterator<D> {
    type Item = MboMessage;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            // Phase M M.A.2: sync bytes_read from CountingReader (closes F-008).
            // Cost: 1 atomic load with Relaxed ordering on x86-64 → ~1 ns/call.
            // BufReader pulls 1MB at a time, so the actual fetch_add inside
            // CountingReader fires only on buffer refill (~17K msgs/refill).
            self.stats.bytes_read = self.bytes_read_handle.load(Ordering::Relaxed);

            // Decode next record
            // decode_record returns a reference to the decoder's internal buffer.
            // We convert immediately, so no clone is needed - the reference is valid
            // until the next decode_record call.
            let dbn_msg_ref = match self.decoder.decode_record::<dbn::MboMsg>() {
                Ok(Some(msg)) => msg,    // Use reference directly (no clone!)
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

#[cfg(feature = "legacy-iterator-api")]
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

// ============================================================================
// Phase M M.A.3 (REV 3): TypedMessageIterator with mid_record_eof + finalize
// ============================================================================

/// Typed iterator over MBO messages yielding `Result<MboMessage, BoundaryError>`.
///
/// Phase M M.A.3 (REV 3 Decision 1): compile-time-enforced error handling at
/// the DBN-loader iterator boundary. Replaces the legacy [`MessageIterator`]
/// (deprecated, gated under `legacy-iterator-api`).
///
/// # EOF semantics (Decision 5b)
///
/// - **Clean EOF** → iterator returns `None`; `is_clean_eof()` is `true`.
/// - **Mid-record EOF** (decoder hits `UnexpectedEof` mid-stream) → iterator
///   returns `None` AND increments `LoaderStats::mid_record_eof` counter;
///   callers MUST check `is_clean_eof()` post-iteration to detect torn streams.
/// - **Decode error** → `Some(Err(BoundaryError::Decode(_)))` (when
///   `skip_invalid=false`); with `skip_invalid=true`, error is logged + counted
///   in `messages_skipped` and iteration continues.
/// - **Convert error** → `Some(Err(BoundaryError::Convert(TlobError)))` (when
///   `skip_invalid=false`); with `skip_invalid=true`, error is logged + counted
///   in `messages_skipped` and iteration continues.
///
/// # finalize() — caller-decided abort/warn
///
/// Call [`Self::finalize`] (consumes the iterator) to retrieve the final
/// `LoaderStats` for `is_clean_eof()` check + observability. Production
/// pipelines (`feature-extractor`) should propagate `Err(...)` on torn streams;
/// analytical callers (`mbo-statistical-profiler`) may WARN + continue.
///
/// # `Arc<AtomicU64>` scope (Decision 17)
///
/// The internal `bytes_read_handle: Arc<AtomicU64>` is single-thread-per-iterator
/// by construction. NEVER share across `Rayon::par_iter` thread boundaries —
/// each parallel-day worker owns its own iterator + its own counter.
///
/// # Example
///
/// ```ignore
/// let loader = DbnLoader::new("data.dbn.zst")?;
/// let mut iter = loader.iter_messages_typed()?;
/// for msg_result in &mut iter {
///     let msg = msg_result?;
///     // ... process ...
/// }
/// let stats = iter.finalize();
/// if !stats.is_clean_eof() {
///     return Err(BoundaryError::Decode(format!(
///         "torn DBN: mid_record_eof={}", stats.mid_record_eof
///     )).into());
/// }
/// ```
pub struct TypedMessageIterator<D: DecodeRecord> {
    decoder: D,
    stats: LoaderStats,
    skip_invalid: bool,
    /// Phase M M.A.2: shared `Arc<AtomicU64>` with the inner [`CountingReader`].
    /// Decision 17: single-thread-per-iterator scope.
    bytes_read_handle: Arc<AtomicU64>,
}

impl<D: DecodeRecord> Iterator for TypedMessageIterator<D> {
    type Item = std::result::Result<MboMessage, BoundaryError>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            // Sync bytes_read from CountingReader on every call (closes F-008).
            self.stats.bytes_read = self.bytes_read_handle.load(Ordering::Relaxed);

            let dbn_msg_ref = match self.decoder.decode_record::<dbn::MboMsg>() {
                Ok(Some(msg)) => msg,
                Ok(None) => return None, // Clean EOF
                Err(e) => {
                    // Phase M Decision 5b: distinguish mid-record EOF from
                    // generic decode error. dbn::Error::Io is a struct variant
                    // with named field `source: io::Error`.
                    let mid_record_eof = matches!(
                        &e,
                        dbn::Error::Io { source, .. }
                            if source.kind() == std::io::ErrorKind::UnexpectedEof
                    );

                    if mid_record_eof {
                        // Stream truncated mid-record. Signal EOF to caller
                        // (returns None) but increment counter so caller can
                        // distinguish via `is_clean_eof()` at finalize() time.
                        self.stats.mid_record_eof += 1;
                        log::warn!(
                            "Mid-record EOF detected: {} (caller should check is_clean_eof)",
                            e
                        );
                        return None;
                    }

                    if self.skip_invalid {
                        log::warn!("Failed to decode DBN record: {e}");
                        self.stats.messages_skipped += 1;
                        continue;
                    }

                    return Some(Err(BoundaryError::Decode(e.to_string())));
                }
            };

            match DbnBridge::convert(dbn_msg_ref) {
                Ok(mbo_msg) => {
                    self.stats.messages_read += 1;
                    return Some(Ok(mbo_msg));
                }
                Err(e) => {
                    if self.skip_invalid {
                        log::warn!("Skipping invalid message: {e}");
                        self.stats.messages_skipped += 1;
                        continue;
                    }

                    return Some(Err(BoundaryError::Convert(e)));
                }
            }
        }
    }
}

impl<D: DecodeRecord> TypedMessageIterator<D> {
    /// Get a reference to the current statistics (live; updated on each `next()`).
    pub fn stats(&self) -> &LoaderStats {
        &self.stats
    }

    /// Get progress as a percentage (0.0 to 100.0).
    ///
    /// Phase M M.A.2: now correctly populated via [`CountingReader`]
    /// (closes F-008 — pre-M.A.2 always returned 0.0 due to `bytes_read=0`).
    pub fn progress(&self) -> f64 {
        if self.stats.file_size == 0 {
            return 100.0;
        }

        (self.stats.bytes_read as f64 / self.stats.file_size as f64) * 100.0
    }

    /// Consume the iterator and return the final [`LoaderStats`].
    ///
    /// Phase M M.A.3 (REV 3 Decision 5c): caller-decides abort/warn.
    ///
    /// After exhausting the iterator (via `for` loop or manual `next()`),
    /// call this method to retrieve the final stats including
    /// `mid_record_eof` counter. Use `LoaderStats::is_clean_eof()` to
    /// distinguish clean EOF from torn-record EOF.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let mut iter = loader.iter_messages_typed()?;
    /// for msg_result in &mut iter {
    ///     let msg = msg_result?;
    ///     // ... process ...
    /// }
    /// let stats = iter.finalize();
    /// if !stats.is_clean_eof() { /* torn stream */ }
    /// ```
    pub fn finalize(mut self) -> LoaderStats {
        // Sync the final bytes_read snapshot one last time before returning.
        self.stats.bytes_read = self.bytes_read_handle.load(Ordering::Relaxed);
        self.stats
    }
}

/// Check if an MBO message represents a valid order (not a system message).
///
/// Returns `false` for system messages (heartbeats, status updates).
///
/// # Deprecated
///
/// Use [`MboMessage::is_system_message()`] instead:
/// ```ignore
/// if !msg.is_system_message() {
///     lob.process_message(&msg)?;
/// }
/// ```
#[deprecated(since = "0.2.0", note = "Use `!msg.is_system_message()` instead")]
#[inline]
pub fn is_valid_order(msg: &MboMessage) -> bool {
    !msg.is_system_message()
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
        std::env::temp_dir().join(format!(
            "loader_test_{}_{}_{}",
            std::process::id(),
            name,
            counter
        ))
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
    fn test_is_system_message() {
        // Valid order is NOT a system message
        let valid = MboMessage::new(123, Action::Add, Side::Bid, 100_000_000_000, 100);
        assert!(!valid.is_system_message());

        // order_id=0 IS a system message
        let sys1 = MboMessage::new(0, Action::Add, Side::Bid, 100_000_000_000, 100);
        assert!(sys1.is_system_message());

        // size=0 IS a system message
        let sys2 = MboMessage::new(123, Action::Add, Side::Bid, 100_000_000_000, 0);
        assert!(sys2.is_system_message());

        // price=0 IS a system message
        let sys3 = MboMessage::new(123, Action::Add, Side::Bid, 0, 100);
        assert!(sys3.is_system_message());

        // price<0 IS a system message
        let sys4 = MboMessage::new(123, Action::Add, Side::Bid, -100, 100);
        assert!(sys4.is_system_message());
    }

    #[test]
    #[allow(deprecated)]
    fn test_is_valid_order_delegates_to_is_system_message() {
        let valid = MboMessage::new(123, Action::Add, Side::Bid, 100_000_000_000, 100);
        assert_eq!(is_valid_order(&valid), !valid.is_system_message());

        let sys = MboMessage::new(0, Action::Add, Side::Bid, 100_000_000_000, 100);
        assert_eq!(is_valid_order(&sys), !sys.is_system_message());
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
