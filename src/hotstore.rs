//! Hot store management for decompressed data files.
//!
//! This module provides efficient management of pre-decompressed data files
//! to eliminate the zstd decompression bottleneck during data processing.
//!
//! # Motivation
//!
//! The main bottleneck in MBO data processing is single-threaded zstd decompression.
//! By pre-decompressing files to a "hot store" directory, we can achieve:
//! - ~5x faster file reading (mmap vs streaming decompression)
//! - Better multi-core utilization (no serial decompression)
//! - Reduced latency for repeated processing
//!
//! # Design Principles
//!
//! - **Format Agnostic**: Works with any file format, not tied to Databento
//! - **Non-Invasive**: Existing code continues to work with compressed files
//! - **Transparent Resolution**: Automatically prefer decompressed when available
//! - **Testable**: All path operations are deterministic and unit-testable
//!
//! # Usage
//!
//! ```ignore
//! use mbo_lob_reconstructor::hotstore::{HotStoreManager, HotStoreConfig};
//!
//! // Create hot store manager
//! let config = HotStoreConfig::dbn_defaults("/data/hot/");
//! let hot_store = HotStoreManager::new(config);
//!
//! // Resolve path - returns decompressed if available, else original
//! let path = hot_store.resolve("/data/raw/NVDA.mbo.dbn.zst");
//!
//! // Pre-decompress files for faster processing
//! hot_store.decompress("/data/raw/NVDA.mbo.dbn.zst")?;
//! ```
//!
//! # Disk Space Considerations
//!
//! Decompressed files are typically 3-4x larger than compressed:
//! - 1 month of NVDA MBO: ~2.5 GB compressed → ~9 GB decompressed
//! - Ensure adequate disk space before decompressing
//!
//! # Thread Safety
//!
//! `HotStoreManager` is `Send + Sync` and can be shared across threads.
//! File decompression uses file-level locking to prevent race conditions.

use std::fs::{self, File};
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

use crate::error::{Result, TlobError};
use crate::loader::IO_BUFFER_SIZE;

// ============================================================================
// Configuration
// ============================================================================

/// Configuration for hot store management.
///
/// Defines how the hot store manager locates and manages decompressed files.
///
/// # Example
///
/// ```
/// use mbo_lob_reconstructor::hotstore::HotStoreConfig;
///
/// // DBN-specific defaults
/// let config = HotStoreConfig::dbn_defaults("/data/hot/");
///
/// // Custom configuration for other formats
/// let config = HotStoreConfig::new("/data/hot/")
///     .with_compressed_ext(".csv.gz")
///     .with_decompressed_ext(".csv");
/// ```
#[derive(Debug, Clone)]
pub struct HotStoreConfig {
    /// Base directory for decompressed files.
    ///
    /// Files will be stored with their original filename (minus compression extension)
    /// in a flat directory structure.
    pub hot_store_dir: PathBuf,

    /// Whether to prefer decompressed files when available.
    ///
    /// When `true`, `resolve()` returns the decompressed path if it exists.
    /// When `false`, always returns the original path.
    pub prefer_decompressed: bool,

    /// File extension for compressed files.
    ///
    /// Used to identify compressed files and derive decompressed filenames.
    /// Example: ".dbn.zst", ".csv.gz"
    pub compressed_ext: String,

    /// File extension for decompressed files.
    ///
    /// The extension used when writing decompressed files.
    /// Example: ".dbn", ".csv"
    pub decompressed_ext: String,
}

impl HotStoreConfig {
    /// Create a new configuration with default settings.
    ///
    /// # Arguments
    ///
    /// * `hot_store_dir` - Directory to store decompressed files
    ///
    /// # Defaults
    ///
    /// - `prefer_decompressed`: true
    /// - `compressed_ext`: ".zst"
    /// - `decompressed_ext`: "" (removes .zst)
    pub fn new<P: AsRef<Path>>(hot_store_dir: P) -> Self {
        Self {
            hot_store_dir: hot_store_dir.as_ref().to_path_buf(),
            prefer_decompressed: true,
            compressed_ext: ".zst".to_string(),
            decompressed_ext: String::new(),
        }
    }

    /// Create configuration with DBN file defaults.
    ///
    /// Configured for `.dbn.zst` → `.dbn` transformation.
    ///
    /// # Arguments
    ///
    /// * `hot_store_dir` - Directory to store decompressed DBN files
    pub fn dbn_defaults<P: AsRef<Path>>(hot_store_dir: P) -> Self {
        Self {
            hot_store_dir: hot_store_dir.as_ref().to_path_buf(),
            prefer_decompressed: true,
            compressed_ext: ".dbn.zst".to_string(),
            decompressed_ext: ".dbn".to_string(),
        }
    }

    /// Set the compressed file extension.
    pub fn with_compressed_ext(mut self, ext: &str) -> Self {
        self.compressed_ext = ext.to_string();
        self
    }

    /// Set the decompressed file extension.
    pub fn with_decompressed_ext(mut self, ext: &str) -> Self {
        self.decompressed_ext = ext.to_string();
        self
    }

    /// Enable or disable preference for decompressed files.
    pub fn with_prefer_decompressed(mut self, prefer: bool) -> Self {
        self.prefer_decompressed = prefer;
        self
    }

    /// Validate the configuration.
    ///
    /// # Returns
    ///
    /// * `Ok(())` - Configuration is valid
    /// * `Err(...)` - Configuration has issues
    pub fn validate(&self) -> Result<()> {
        if self.hot_store_dir.as_os_str().is_empty() {
            return Err(TlobError::InvalidConfig(
                "HotStoreConfig.hot_store_dir cannot be empty".into(),
            ));
        }
        if self.compressed_ext.is_empty() {
            return Err(TlobError::InvalidConfig(
                "HotStoreConfig.compressed_ext cannot be empty".into(),
            ));
        }
        Ok(())
    }
}

impl Default for HotStoreConfig {
    fn default() -> Self {
        Self::new("./hot_store")
    }
}

// ============================================================================
// Hot Store Manager
// ============================================================================

/// Manages decompressed "hot" files for faster I/O.
///
/// The hot store manager provides transparent resolution between compressed
/// and decompressed file paths, enabling faster data processing by avoiding
/// the zstd decompression bottleneck.
///
/// # Example
///
/// ```ignore
/// use mbo_lob_reconstructor::hotstore::{HotStoreManager, HotStoreConfig};
///
/// let manager = HotStoreManager::new(HotStoreConfig::dbn_defaults("/data/hot/"));
///
/// // Resolve automatically prefers decompressed if available
/// let path = manager.resolve("/data/raw/NVDA.mbo.dbn.zst");
///
/// // Check availability
/// if manager.has_decompressed("/data/raw/NVDA.mbo.dbn.zst") {
///     println!("Using fast path!");
/// }
///
/// // Pre-decompress for future runs
/// manager.decompress("/data/raw/NVDA.mbo.dbn.zst")?;
/// ```
#[derive(Debug, Clone)]
pub struct HotStoreManager {
    config: HotStoreConfig,
}

impl HotStoreManager {
    /// Create a new hot store manager.
    ///
    /// # Arguments
    ///
    /// * `config` - Configuration for the hot store
    ///
    /// # Note
    ///
    /// The hot store directory will be created lazily on first decompress.
    pub fn new(config: HotStoreConfig) -> Self {
        Self { config }
    }

    /// Create a hot store manager with DBN defaults.
    ///
    /// Convenience constructor for the common case of DBN files.
    ///
    /// # Arguments
    ///
    /// * `hot_store_dir` - Directory to store decompressed files
    pub fn for_dbn<P: AsRef<Path>>(hot_store_dir: P) -> Self {
        Self::new(HotStoreConfig::dbn_defaults(hot_store_dir))
    }

    /// Get the configuration.
    pub fn config(&self) -> &HotStoreConfig {
        &self.config
    }

    /// Resolve a file path to its best available form.
    ///
    /// If `prefer_decompressed` is true and a decompressed version exists
    /// in the hot store, returns that path. Otherwise returns the original path.
    ///
    /// # Arguments
    ///
    /// * `path` - Path to a (possibly compressed) file
    ///
    /// # Returns
    ///
    /// The resolved path - either decompressed (if available) or original.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let manager = HotStoreManager::for_dbn("/data/hot/");
    ///
    /// // If /data/hot/NVDA.mbo.dbn exists:
    /// let resolved = manager.resolve("/data/raw/NVDA.mbo.dbn.zst");
    /// assert_eq!(resolved, Path::new("/data/hot/NVDA.mbo.dbn"));
    ///
    /// // If decompressed doesn't exist:
    /// let resolved = manager.resolve("/data/raw/OTHER.mbo.dbn.zst");
    /// assert_eq!(resolved, Path::new("/data/raw/OTHER.mbo.dbn.zst"));
    /// ```
    pub fn resolve<P: AsRef<Path>>(&self, path: P) -> PathBuf {
        let path = path.as_ref();

        // If preference disabled, return original
        if !self.config.prefer_decompressed {
            return path.to_path_buf();
        }

        // Check if decompressed version exists
        let decompressed = self.decompressed_path_for(path);
        if decompressed.exists() {
            decompressed
        } else {
            path.to_path_buf()
        }
    }

    /// Check if a decompressed version exists in the hot store.
    ///
    /// # Arguments
    ///
    /// * `compressed_path` - Path to the compressed file
    ///
    /// # Returns
    ///
    /// `true` if the decompressed file exists in the hot store.
    pub fn has_decompressed<P: AsRef<Path>>(&self, compressed_path: P) -> bool {
        self.decompressed_path_for(compressed_path).exists()
    }

    /// Get the decompressed path for a compressed file.
    ///
    /// Returns the path where the decompressed file would be stored,
    /// regardless of whether it actually exists.
    ///
    /// # Arguments
    ///
    /// * `compressed_path` - Path to the compressed file
    ///
    /// # Returns
    ///
    /// The path in the hot store where the decompressed file would be located.
    pub fn decompressed_path_for<P: AsRef<Path>>(&self, compressed_path: P) -> PathBuf {
        let compressed_path = compressed_path.as_ref();

        // Extract filename
        let filename = compressed_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("");

        // Transform filename: remove compressed extension, add decompressed extension
        let new_filename = if filename.ends_with(&self.config.compressed_ext) {
            let base = &filename[..filename.len() - self.config.compressed_ext.len()];
            format!("{}{}", base, self.config.decompressed_ext)
        } else {
            // If not compressed, just use original filename
            filename.to_string()
        };

        self.config.hot_store_dir.join(new_filename)
    }

    /// Decompress a file to the hot store.
    ///
    /// Uses zstd streaming decompression with optimized buffer sizes.
    ///
    /// # Arguments
    ///
    /// * `compressed_path` - Path to the compressed file
    ///
    /// # Returns
    ///
    /// * `Ok(PathBuf)` - Path to the decompressed file
    /// * `Err(...)` - Decompression failed
    ///
    /// # Thread Safety
    ///
    /// Uses atomic file operations to handle concurrent decompression attempts.
    /// If two threads try to decompress the same file, one will succeed and
    /// the other will find the file already exists.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let manager = HotStoreManager::for_dbn("/data/hot/");
    /// let decompressed = manager.decompress("/data/raw/NVDA.mbo.dbn.zst")?;
    /// println!("Decompressed to: {:?}", decompressed);
    /// ```
    pub fn decompress<P: AsRef<Path>>(&self, compressed_path: P) -> Result<PathBuf> {
        let compressed_path = compressed_path.as_ref();
        let decompressed_path = self.decompressed_path_for(compressed_path);

        // Fast path: already decompressed
        if decompressed_path.exists() {
            log::info!(
                "Decompressed file already exists: {}",
                decompressed_path.display()
            );
            return Ok(decompressed_path);
        }

        // Ensure hot store directory exists (idempotent across threads;
        // multiple callers racing on this is fine — fs::create_dir_all
        // returns Ok if the directory already exists).
        let parent_dir = decompressed_path.parent().ok_or_else(|| {
            TlobError::generic(format!(
                "Decompressed path has no parent: {}",
                decompressed_path.display()
            ))
        })?;
        fs::create_dir_all(parent_dir).map_err(|e| {
            TlobError::generic(format!(
                "Failed to create hot store directory {}: {}",
                parent_dir.display(),
                e
            ))
        })?;

        // Phase O Cycle 1 / B.4: race-safe atomic write via NamedTempFile
        // + persist_noclobber. Pre-B.4 used a fixed `.tmp` extension
        // which exhibited four distinct failure modes:
        //
        //   (a) TOCTOU race: two threads both pass the `.exists()` check
        //       above, both decompress to the SAME `.tmp` path → last-
        //       writer-wins corruption (one thread's write is overwritten
        //       mid-stream by the other's).
        //   (b) Cross-input collision: different compressed inputs that
        //       map to the same `decompressed_path` (e.g., basename-only
        //       hashing in `decompressed_path_for` — see C-1 in B.4
        //       pre-impl audit; out of B.4 scope, deferred to Phase P)
        //       collide on the SAME `.tmp` path with the same corruption.
        //   (c) Last-writer-wins on `fs::rename`: process A renames first;
        //       process B's rename then OVERWRITES A's clean output with
        //       B's identical-but-newly-written file (low risk but pointless
        //       I/O + orphan inode churn).
        //   (d) Sync-targeting bug (D-2 from B.4 pre-impl audit): the
        //       pre-B.4 `decompress_zstd(src, dst: &Path)` opened a NEW
        //       file at `dst` via `File::create(dst)`, which truncates
        //       and replaces whatever inode was at that path. Any caller
        //       that subsequently called `tmp.as_file().sync_all()` would
        //       have synced the ORIGINAL (now-empty) inode, NOT the new
        //       inode containing the decompressed bytes — atomicity
        //       illusory. Post-B.4 `decompress_zstd(src, dst_file: &mut
        //       File)` writes through `tmp.as_file_mut()` so `sync_all`
        //       targets the SAME inode that received the bytes.
        //
        // M.A.5 envelope writer at `src/lob/reconstructor.rs:366-388` is
        // the architectural template for the sync_all + persist sequence
        // (M.A.5 uses `persist` for single-writer JSON envelope; B.4
        // uses `persist_noclobber` for race-safe multi-writer where the
        // FIRST winner persists and subsequent threads detect via
        // `ErrorKind::AlreadyExists`).
        //
        // `tempfile::NamedTempFile::persist_noclobber` is documented as
        // NON-ATOMIC on systems where `renameat_with(NOREPLACE)` returns
        // EINVAL (rare; primary path uses `linkat` + `unlinkat` on POSIX
        // and `MoveFileEx` without REPLACE_EXISTING on Windows). On macOS
        // APFS (the typical hot store mount per CLAUDE.md), the primary
        // path IS atomic. Operators on filesystems that fall back to the
        // non-atomic path may rarely see two valid copies briefly during
        // concurrent decompression — both copies have identical content
        // (deterministic zstd output), so the consumer sees no corruption.
        let mut temp = tempfile::NamedTempFile::new_in(parent_dir).map_err(|e| {
            TlobError::generic(format!(
                "Failed to create temp file in {}: {}",
                parent_dir.display(),
                e
            ))
        })?;

        log::info!(
            "Decompressing {} to {}",
            compressed_path.display(),
            decompressed_path.display()
        );

        // Phase O Cycle 1 / B.4 D-2 fix: decompress_zstd writes through
        // the NamedTempFile's own File handle (not via File::create on
        // the path). This guarantees the bytes land in the inode that
        // `temp.as_file().sync_all()` will subsequently fsync.
        let bytes = self.decompress_zstd(compressed_path, temp.as_file_mut())?;

        // Phase O Cycle 1 / B.4 C-3/D-1 fix: fsync the SAME inode that
        // just received the decompressed bytes, matching the M.A.5
        // envelope-writer idiom at reconstructor.rs:369. Without this,
        // a power-cut between persist_noclobber and the OS's lazy flush
        // of the page cache could leave the renamed file pointing at
        // zeroed or torn data on disk (rename is metadata-atomic but
        // data blocks may not be on disk yet at rename time).
        temp.as_file().sync_all().map_err(|e| {
            TlobError::generic(format!(
                "Failed to fsync decompressed temp file before persist: {}",
                e
            ))
        })?;

        match temp.persist_noclobber(&decompressed_path) {
            Ok(_) => {
                log::info!("Decompression complete: {} bytes written", bytes);
                // Phase O Cycle 1 / B.4 post-rename integrity check:
                // catches the edge case where decompress_zstd returned
                // Ok(0) (zstd source had a valid header but zero
                // decompressible content — e.g., empty zstd frame).
                // Per HFT-rules §8 (fail-loud, never silently emit
                // empty/corrupt output).
                let metadata = fs::metadata(&decompressed_path).map_err(|e| {
                    TlobError::generic(format!(
                        "Post-decompress metadata read failed for {}: {}",
                        decompressed_path.display(),
                        e
                    ))
                })?;
                if metadata.len() == 0 {
                    return Err(TlobError::generic(format!(
                        "Decompression produced empty file at {} \
                         (zstd source likely contained zero decompressible \
                          content despite valid header)",
                        decompressed_path.display()
                    )));
                }
                Ok(decompressed_path)
            }
            Err(persist_err) => {
                // Phase O Cycle 1 / B.4 D-3 fix: discriminate the
                // persist_noclobber Err — only ErrorKind::AlreadyExists
                // permits the "another thread won" recovery branch. Any
                // other Err (EROFS, EACCES, EXDEV, ENOSPC, ...) MUST
                // propagate; pre-B.4 design used a bare `.exists()`
                // check which would silently mask EROFS as success
                // when a stale orphan happened to exist at the target.
                let kind = persist_err.error.kind();
                if kind == io::ErrorKind::AlreadyExists && decompressed_path.exists() {
                    log::info!(
                        "Concurrent decompression detected for {}; \
                         using existing file (another thread won the race)",
                        decompressed_path.display()
                    );
                    Ok(decompressed_path)
                } else {
                    Err(TlobError::generic(format!(
                        "Failed to persist decompressed temp to {} \
                         (kind={:?}): {}",
                        decompressed_path.display(),
                        kind,
                        persist_err.error
                    )))
                }
            }
        }
    }

    /// Internal zstd decompression implementation.
    ///
    /// # Phase O Cycle 1 / B.4 (D-2 fix)
    ///
    /// Pre-B.4 signature was `(&self, src: &Path, dst: &Path) -> Result<u64>`
    /// and the function called `File::create(dst)` to obtain a write
    /// handle, opening a NEW inode at the temp path. The caller's
    /// `NamedTempFile` handle (`tmp.as_file()`) was bound to the
    /// ORIGINAL inode created by `NamedTempFile::new_in()`, so any
    /// subsequent `tmp.as_file().sync_all()` would have synced the
    /// WRONG (now-empty) inode rather than the one containing the
    /// decompressed bytes.
    ///
    /// Post-B.4 the signature accepts `dst_file: &mut File` and writes
    /// through that handle directly. The caller passes
    /// `tmp.as_file_mut()` so `tmp.as_file().sync_all()` correctly
    /// targets the same inode that received the bytes.
    fn decompress_zstd(&self, src: &Path, dst_file: &mut File) -> Result<u64> {
        let input_file = File::open(src)
            .map_err(|e| TlobError::generic(format!("Failed to open {}: {}", src.display(), e)))?;

        // Use large buffers for better throughput. BufWriter borrows
        // dst_file mutably; the borrow ends when BufWriter is dropped
        // at function return, releasing dst_file for the caller's
        // sync_all call.
        let reader = BufReader::with_capacity(IO_BUFFER_SIZE, input_file);
        let mut writer = BufWriter::with_capacity(IO_BUFFER_SIZE, dst_file);

        // Create zstd decoder
        let mut decoder = zstd::stream::read::Decoder::new(reader)
            .map_err(|e| TlobError::generic(format!("Failed to create zstd decoder: {}", e)))?;

        // Stream decompress
        let mut buffer = vec![0u8; IO_BUFFER_SIZE];
        let mut total_bytes = 0u64;

        loop {
            let bytes_read = decoder
                .read(&mut buffer)
                .map_err(|e| TlobError::generic(format!("Decompression read error: {}", e)))?;

            if bytes_read == 0 {
                break;
            }

            writer
                .write_all(&buffer[..bytes_read])
                .map_err(|e| TlobError::generic(format!("Decompression write error: {}", e)))?;

            total_bytes += bytes_read as u64;
        }

        writer
            .flush()
            .map_err(|e| TlobError::generic(format!("Failed to flush output: {}", e)))?;

        Ok(total_bytes)
    }

    /// List all decompressed files in the hot store.
    ///
    /// # Returns
    ///
    /// * `Ok(Vec<PathBuf>)` - List of decompressed file paths
    /// * `Err(...)` - Failed to read directory
    pub fn list_hot_files(&self) -> Result<Vec<PathBuf>> {
        if !self.config.hot_store_dir.exists() {
            return Ok(Vec::new());
        }

        let mut files = Vec::new();

        for entry in fs::read_dir(&self.config.hot_store_dir)
            .map_err(|e| TlobError::generic(format!("Failed to read hot store directory: {}", e)))?
        {
            let entry = entry.map_err(|e| {
                TlobError::generic(format!("Failed to read directory entry: {}", e))
            })?;

            let path = entry.path();

            // Filter by decompressed extension if specified
            if !self.config.decompressed_ext.is_empty() {
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    if name.ends_with(&self.config.decompressed_ext) {
                        files.push(path);
                    }
                }
            } else if path.is_file() {
                files.push(path);
            }
        }

        files.sort();
        Ok(files)
    }

    /// Get total size of hot store in bytes.
    ///
    /// # Returns
    ///
    /// Total size of all files in the hot store directory.
    pub fn hot_store_size(&self) -> io::Result<u64> {
        if !self.config.hot_store_dir.exists() {
            return Ok(0);
        }

        let mut total = 0u64;

        for entry in fs::read_dir(&self.config.hot_store_dir)? {
            let entry = entry?;
            if entry.path().is_file() {
                total += entry.metadata()?.len();
            }
        }

        Ok(total)
    }

    /// Remove all files from the hot store.
    ///
    /// # Warning
    ///
    /// This permanently deletes all decompressed files!
    pub fn clear(&self) -> Result<()> {
        if !self.config.hot_store_dir.exists() {
            return Ok(());
        }

        for entry in fs::read_dir(&self.config.hot_store_dir)
            .map_err(|e| TlobError::generic(format!("Failed to read hot store: {}", e)))?
        {
            let entry =
                entry.map_err(|e| TlobError::generic(format!("Failed to read entry: {}", e)))?;

            let path = entry.path();
            if path.is_file() {
                fs::remove_file(&path).map_err(|e| {
                    TlobError::generic(format!("Failed to remove {}: {}", path.display(), e))
                })?;
            }
        }

        Ok(())
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};

    // Unique counter for test isolation
    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn unique_temp_dir(test_name: &str) -> PathBuf {
        let counter = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
        std::env::temp_dir().join(format!(
            "hotstore_test_{}_{}_{}",
            std::process::id(),
            test_name,
            counter
        ))
    }

    fn cleanup(dir: &Path) {
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn test_config_new() {
        let config = HotStoreConfig::new("/data/hot");
        assert_eq!(config.hot_store_dir, PathBuf::from("/data/hot"));
        assert!(config.prefer_decompressed);
        assert_eq!(config.compressed_ext, ".zst");
        assert_eq!(config.decompressed_ext, "");
    }

    #[test]
    fn test_config_dbn_defaults() {
        let config = HotStoreConfig::dbn_defaults("/data/hot");
        assert_eq!(config.compressed_ext, ".dbn.zst");
        assert_eq!(config.decompressed_ext, ".dbn");
    }

    #[test]
    fn test_config_builder() {
        let config = HotStoreConfig::new("/data/hot")
            .with_compressed_ext(".csv.gz")
            .with_decompressed_ext(".csv")
            .with_prefer_decompressed(false);

        assert_eq!(config.compressed_ext, ".csv.gz");
        assert_eq!(config.decompressed_ext, ".csv");
        assert!(!config.prefer_decompressed);
    }

    #[test]
    fn test_config_validation() {
        let config = HotStoreConfig::dbn_defaults("/data/hot");
        assert!(config.validate().is_ok());

        let bad_config = HotStoreConfig {
            hot_store_dir: PathBuf::new(),
            ..HotStoreConfig::default()
        };
        assert!(bad_config.validate().is_err());

        let bad_config = HotStoreConfig::new("/data").with_compressed_ext("");
        assert!(bad_config.validate().is_err());
    }

    #[test]
    fn test_manager_decompressed_path_for_dbn() {
        let manager = HotStoreManager::for_dbn("/data/hot");

        // Standard case
        let result = manager.decompressed_path_for("/raw/NVDA.mbo.dbn.zst");
        assert_eq!(result, PathBuf::from("/data/hot/NVDA.mbo.dbn"));

        // Different path depth
        let result = manager.decompressed_path_for("/a/b/c/DATA.mbo.dbn.zst");
        assert_eq!(result, PathBuf::from("/data/hot/DATA.mbo.dbn"));

        // Already decompressed (no transformation)
        let result = manager.decompressed_path_for("/raw/NVDA.mbo.dbn");
        assert_eq!(result, PathBuf::from("/data/hot/NVDA.mbo.dbn"));
    }

    #[test]
    fn test_manager_decompressed_path_for_custom() {
        let config = HotStoreConfig::new("/data/hot")
            .with_compressed_ext(".csv.gz")
            .with_decompressed_ext(".csv");
        let manager = HotStoreManager::new(config);

        let result = manager.decompressed_path_for("/raw/data.csv.gz");
        assert_eq!(result, PathBuf::from("/data/hot/data.csv"));
    }

    #[test]
    fn test_resolve_prefers_decompressed() {
        let dir = unique_temp_dir("resolve_prefers");
        cleanup(&dir);

        // Create hot store directory
        let hot_dir = dir.join("hot");
        fs::create_dir_all(&hot_dir).unwrap();

        // Create fake decompressed file
        let decompressed = hot_dir.join("test.mbo.dbn");
        fs::write(&decompressed, "test data").unwrap();

        let manager = HotStoreManager::for_dbn(&hot_dir);

        // Should resolve to decompressed
        let resolved = manager.resolve("/raw/test.mbo.dbn.zst");
        assert_eq!(resolved, decompressed);

        // Non-existent should return original
        let resolved = manager.resolve("/raw/other.mbo.dbn.zst");
        assert_eq!(resolved, PathBuf::from("/raw/other.mbo.dbn.zst"));

        cleanup(&dir);
    }

    #[test]
    fn test_resolve_respects_preference() {
        let dir = unique_temp_dir("resolve_respects");
        cleanup(&dir);

        // Create hot store directory
        let hot_dir = dir.join("hot");
        fs::create_dir_all(&hot_dir).unwrap();

        // Create fake decompressed file
        let decompressed = hot_dir.join("test.mbo.dbn");
        fs::write(&decompressed, "test data").unwrap();

        // With preference disabled
        let config = HotStoreConfig::dbn_defaults(&hot_dir).with_prefer_decompressed(false);
        let manager = HotStoreManager::new(config);

        // Should return original even though decompressed exists
        let resolved = manager.resolve("/raw/test.mbo.dbn.zst");
        assert_eq!(resolved, PathBuf::from("/raw/test.mbo.dbn.zst"));

        cleanup(&dir);
    }

    #[test]
    fn test_has_decompressed() {
        let dir = unique_temp_dir("has_decompressed");
        cleanup(&dir);

        let hot_dir = dir.join("hot");
        fs::create_dir_all(&hot_dir).unwrap();

        let decompressed = hot_dir.join("test.mbo.dbn");
        fs::write(&decompressed, "test data").unwrap();

        let manager = HotStoreManager::for_dbn(&hot_dir);

        assert!(manager.has_decompressed("/raw/test.mbo.dbn.zst"));
        assert!(!manager.has_decompressed("/raw/other.mbo.dbn.zst"));

        cleanup(&dir);
    }

    #[test]
    fn test_list_hot_files() {
        let dir = unique_temp_dir("list_hot_files");
        cleanup(&dir);

        let hot_dir = dir.join("hot");
        fs::create_dir_all(&hot_dir).unwrap();

        // Create some files
        fs::write(hot_dir.join("a.mbo.dbn"), "data a").unwrap();
        fs::write(hot_dir.join("b.mbo.dbn"), "data b").unwrap();
        fs::write(hot_dir.join("other.txt"), "other").unwrap();

        let manager = HotStoreManager::for_dbn(&hot_dir);
        let files = manager.list_hot_files().unwrap();

        // Should only include .dbn files
        assert_eq!(files.len(), 2);
        assert!(files[0]
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .contains("a.mbo.dbn"));
        assert!(files[1]
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .contains("b.mbo.dbn"));

        cleanup(&dir);
    }

    #[test]
    fn test_hot_store_size() {
        let dir = unique_temp_dir("hot_store_size");
        cleanup(&dir);

        let hot_dir = dir.join("hot");
        fs::create_dir_all(&hot_dir).unwrap();

        // Create files with known sizes
        fs::write(hot_dir.join("a.dbn"), vec![0u8; 1000]).unwrap();
        fs::write(hot_dir.join("b.dbn"), vec![0u8; 2000]).unwrap();

        let manager = HotStoreManager::for_dbn(&hot_dir);
        let size = manager.hot_store_size().unwrap();

        assert_eq!(size, 3000);

        cleanup(&dir);
    }

    #[test]
    fn test_clear() {
        let dir = unique_temp_dir("clear");
        cleanup(&dir);

        let hot_dir = dir.join("hot");
        fs::create_dir_all(&hot_dir).unwrap();

        fs::write(hot_dir.join("a.dbn"), "data a").unwrap();
        fs::write(hot_dir.join("b.dbn"), "data b").unwrap();

        let manager = HotStoreManager::for_dbn(&hot_dir);
        manager.clear().unwrap();

        let files = manager.list_hot_files().unwrap();
        assert!(files.is_empty());

        // Directory should still exist
        assert!(hot_dir.exists());

        cleanup(&dir);
    }

    #[test]
    fn test_decompress_creates_directory() {
        let dir = unique_temp_dir("decompress_creates");
        cleanup(&dir);

        let hot_dir = dir.join("hot");
        let manager = HotStoreManager::for_dbn(&hot_dir);

        // Create a compressed file
        let compressed_dir = dir.join("compressed");
        fs::create_dir_all(&compressed_dir).unwrap();

        let compressed_path = compressed_dir.join("test.dbn.zst");
        let test_data = b"Hello, hot store!";

        // Compress test data
        let mut encoder =
            zstd::stream::write::Encoder::new(File::create(&compressed_path).unwrap(), 0).unwrap();
        encoder.write_all(test_data).unwrap();
        encoder.finish().unwrap();

        // Decompress
        let decompressed = manager.decompress(&compressed_path).unwrap();

        // Verify
        assert!(decompressed.exists());
        assert!(hot_dir.exists());
        let content = fs::read(&decompressed).unwrap();
        assert_eq!(content, test_data);

        cleanup(&dir);
    }

    #[test]
    fn test_decompress_idempotent() {
        let dir = unique_temp_dir("decompress_idempotent");
        cleanup(&dir);

        let hot_dir = dir.join("hot");
        let manager = HotStoreManager::for_dbn(&hot_dir);

        // Create compressed file
        let compressed_dir = dir.join("compressed");
        fs::create_dir_all(&compressed_dir).unwrap();

        let compressed_path = compressed_dir.join("test.dbn.zst");
        let test_data = b"Test data for idempotency";

        let mut encoder =
            zstd::stream::write::Encoder::new(File::create(&compressed_path).unwrap(), 0).unwrap();
        encoder.write_all(test_data).unwrap();
        encoder.finish().unwrap();

        // Decompress twice
        let result1 = manager.decompress(&compressed_path).unwrap();
        let result2 = manager.decompress(&compressed_path).unwrap();

        // Should return same path
        assert_eq!(result1, result2);

        cleanup(&dir);
    }

    // ─────────────────────────────────────────────────────────────────
    // Phase O Cycle 1 / B.4 — Hot-store atomicity tests
    //
    // These tests verify the post-B.4 race-safety + integrity invariants:
    //   - tempfile::NamedTempFile::new_in eliminates fixed `.tmp`
    //     collision class (T-1 concurrent_same_path + concurrent_distinct_basenames)
    //   - persist_noclobber + ErrorKind::AlreadyExists discriminator
    //     handles "another thread won" without masking real errors
    //   - Post-rename metadata().len() > 0 fails loud on zero-byte
    //     decompression output (T-1 truncated_source)
    //   - HotStoreManager: Send + Sync at the type level (T-1 send_sync)
    // ─────────────────────────────────────────────────────────────────

    /// B.4 concurrent same-path: 8 threads decompress the same compressed
    /// input simultaneously. Verifies persist_noclobber's race safety
    /// (only ONE thread persists the file; the other 7 detect via
    /// ErrorKind::AlreadyExists and use the existing file). Asserts:
    ///   - all threads return Ok with the SAME path,
    ///   - file content matches the original test_data,
    ///   - no `.tmp*` orphan files remain in hot_dir (NamedTempFile RAII
    ///     cleanup on losing-thread Drop).
    #[test]
    fn b_4_decompress_concurrent_same_path_no_corruption() {
        use std::sync::{Arc, Barrier};
        use std::thread;

        let dir = unique_temp_dir("b4_concurrent_same");
        cleanup(&dir);

        let hot_dir = dir.join("hot");
        let manager = HotStoreManager::for_dbn(&hot_dir);

        let compressed_dir = dir.join("compressed");
        fs::create_dir_all(&compressed_dir).unwrap();
        let compressed_path = compressed_dir.join("test.dbn.zst");
        // 10KB pseudo-random content (deterministic per i % 256) so
        // content-equality assertion is meaningful.
        let test_data: Vec<u8> = (0..10_000).map(|i| (i % 256) as u8).collect();

        let mut encoder =
            zstd::stream::write::Encoder::new(File::create(&compressed_path).unwrap(), 0).unwrap();
        encoder.write_all(&test_data).unwrap();
        encoder.finish().unwrap();

        let manager_arc = Arc::new(manager);
        let compressed_arc = Arc::new(compressed_path);
        // Barrier aligns thread starts so all 8 threads cross the
        // `.exists()` check at near-identical instants — maximizes
        // race-window probability.
        let barrier = Arc::new(Barrier::new(8));

        let handles: Vec<_> = (0..8)
            .map(|_| {
                let m = manager_arc.clone();
                let p = compressed_arc.clone();
                let b = barrier.clone();
                thread::spawn(move || {
                    b.wait();
                    m.decompress(&*p)
                })
            })
            .collect();

        let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();

        // All 8 threads MUST succeed (winning thread persists; losing
        // threads recover via ErrorKind::AlreadyExists).
        for (i, r) in results.iter().enumerate() {
            assert!(r.is_ok(), "thread {i} failed: {r:?}");
        }

        // All threads MUST return the SAME path (winner's path is the
        // canonical decompressed_path; losers return that same path
        // via the recovery branch).
        let first_path = results[0].as_ref().unwrap();
        for (i, r) in results[1..].iter().enumerate() {
            assert_eq!(
                r.as_ref().unwrap(),
                first_path,
                "thread {} returned different path",
                i + 1
            );
        }

        // File content MUST match the original — no torn writes from
        // concurrent persist attempts.
        let content = fs::read(first_path).unwrap();
        assert_eq!(
            content, test_data,
            "decompressed content must match original"
        );

        // No `.tmp*` orphans should remain in hot_dir. Losing threads'
        // NamedTempFile drops on the persist_noclobber Err path and
        // RAII unlinks the temp inode automatically.
        let orphans: Vec<_> = fs::read_dir(&hot_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().into_string().unwrap_or_default())
            .filter(|n| n.starts_with(".tmp") || n.contains(".tmp"))
            .collect();
        assert!(
            orphans.is_empty(),
            "no .tmp* orphans should remain post-decompression; found: {orphans:?}"
        );

        cleanup(&dir);
    }

    /// B.4 concurrent distinct basenames: 4 threads each decompress a
    /// DIFFERENT compressed input (4 distinct basenames → 4 distinct
    /// decompressed paths → 4 distinct NamedTempFile temp files). Verifies
    /// no cross-input contamination (random-suffix temp naming via
    /// NamedTempFile::new_in eliminates the cross-input `.tmp` collision
    /// class that the pre-B.4 fixed `.tmp` extension was vulnerable to).
    #[test]
    fn b_4_decompress_concurrent_distinct_basenames() {
        use std::sync::{Arc, Barrier};
        use std::thread;

        let dir = unique_temp_dir("b4_concurrent_distinct");
        cleanup(&dir);

        let hot_dir = dir.join("hot");
        let manager = HotStoreManager::for_dbn(&hot_dir);

        let compressed_dir = dir.join("compressed");
        fs::create_dir_all(&compressed_dir).unwrap();

        // 4 distinct compressed inputs with DISTINCT content (so cross-
        // contamination from .tmp collision would produce a content
        // mismatch we can detect).
        let inputs: Vec<(PathBuf, Vec<u8>)> = (0..4)
            .map(|i| {
                let path = compressed_dir.join(format!("input_{i}.dbn.zst"));
                let content: Vec<u8> = (0..5_000).map(|j| ((i * 1000 + j) % 256) as u8).collect();
                let mut encoder =
                    zstd::stream::write::Encoder::new(File::create(&path).unwrap(), 0).unwrap();
                encoder.write_all(&content).unwrap();
                encoder.finish().unwrap();
                (path, content)
            })
            .collect();

        let manager_arc = Arc::new(manager);
        let inputs_arc = Arc::new(inputs);
        let barrier = Arc::new(Barrier::new(4));

        let handles: Vec<_> = (0..4)
            .map(|i| {
                let m = manager_arc.clone();
                let inp = inputs_arc.clone();
                let b = barrier.clone();
                thread::spawn(move || {
                    b.wait();
                    let result = m.decompress(&inp[i].0);
                    (i, result)
                })
            })
            .collect();

        let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();

        // Each thread must return Ok with the CORRECT content for its
        // input. If cross-input contamination occurred (e.g., thread 0
        // and thread 2 both wrote to the SAME .tmp path due to basename
        // collision), the content read back would be wrong.
        for (idx, r) in results {
            let path = r.as_ref().unwrap_or_else(|e| panic!("thread {idx}: {e:?}"));
            let content = fs::read(path).unwrap();
            assert_eq!(
                content, inputs_arc[idx].1,
                "thread {idx}: content mismatch — possible cross-input \
                 contamination from .tmp collision"
            );
        }

        cleanup(&dir);
    }

    /// B.4 truncated source: a corrupt zstd source (header + zero
    /// decompressible bytes) MUST fail-loud at the post-rename
    /// metadata().len() > 0 integrity check, NOT silently return
    /// the path of a zero-byte file. Per HFT-rules §8.
    #[test]
    fn b_4_decompress_zero_byte_source_returns_err_not_empty_file() {
        let dir = unique_temp_dir("b4_truncated");
        cleanup(&dir);

        let hot_dir = dir.join("hot");
        let manager = HotStoreManager::for_dbn(&hot_dir);

        let compressed_dir = dir.join("compressed");
        fs::create_dir_all(&compressed_dir).unwrap();
        let compressed_path = compressed_dir.join("empty.dbn.zst");

        // Compress ZERO bytes → valid zstd frame containing no
        // decompressible content. This is the edge case where
        // decompress_zstd returns Ok(0) but the resulting file is
        // empty — pre-B.4 would have silently returned the zero-byte
        // path; post-B.4 the metadata().len() == 0 check fails loud.
        // `encoder` is bound non-mutably because no write_all() call
        // precedes finish() — finish() consumes self by value, no
        // mutation occurs (B.4 post-impl validator MED-1 closure;
        // sibling concurrent_* tests at the same scope correctly use
        // `let mut encoder` because they DO call write_all() before
        // finish, which takes &mut self).
        let encoder =
            zstd::stream::write::Encoder::new(File::create(&compressed_path).unwrap(), 0).unwrap();
        // No write_all() call — just finish to emit an empty frame.
        encoder.finish().unwrap();

        let result = manager.decompress(&compressed_path);

        assert!(
            result.is_err(),
            "zero-byte zstd source MUST fail-loud (HFT-rules §8); got: {result:?}"
        );
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("empty file"),
            "error message must cite empty-file failure mode; got: {err_msg}"
        );

        cleanup(&dir);
    }

    /// B.4 Send + Sync static assertion (T-1): HotStoreManager + its
    /// internal config must be safely shareable across threads. The
    /// `concurrent_*` tests above rely on this via Arc<HotStoreManager>;
    /// this test asserts the trait bounds at the TYPE level so a future
    /// refactor that accidentally adds a non-Send/Sync field (e.g., Rc,
    /// RefCell) fires a compile error rather than silently breaking
    /// the concurrent tests.
    #[test]
    fn b_4_send_sync_static_assertions() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<HotStoreManager>();
        assert_send_sync::<HotStoreConfig>();
    }
}
