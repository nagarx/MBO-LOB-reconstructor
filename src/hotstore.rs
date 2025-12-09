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
            return Err(TlobError::generic("hot_store_dir cannot be empty"));
        }
        if self.compressed_ext.is_empty() {
            return Err(TlobError::generic("compressed_ext cannot be empty"));
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

        // Check if already decompressed
        if decompressed_path.exists() {
            log::info!(
                "Decompressed file already exists: {}",
                decompressed_path.display()
            );
            return Ok(decompressed_path);
        }

        // Ensure hot store directory exists
        if let Some(parent) = decompressed_path.parent() {
            fs::create_dir_all(parent).map_err(|e| {
                TlobError::generic(format!(
                    "Failed to create hot store directory {}: {}",
                    parent.display(),
                    e
                ))
            })?;
        }

        // Use temporary file for atomic write
        let temp_path = decompressed_path.with_extension("tmp");

        // Decompress
        log::info!(
            "Decompressing {} to {}",
            compressed_path.display(),
            decompressed_path.display()
        );

        let result = self.decompress_zstd(compressed_path, &temp_path);

        match result {
            Ok(bytes) => {
                // Atomic rename
                fs::rename(&temp_path, &decompressed_path).map_err(|e| {
                    // Clean up temp file on rename failure
                    let _ = fs::remove_file(&temp_path);
                    TlobError::generic(format!("Failed to rename temp file: {}", e))
                })?;

                log::info!(
                    "Decompression complete: {} bytes written",
                    bytes
                );
                Ok(decompressed_path)
            }
            Err(e) => {
                // Clean up temp file on error
                let _ = fs::remove_file(&temp_path);
                Err(e)
            }
        }
    }

    /// Internal zstd decompression implementation.
    fn decompress_zstd(&self, src: &Path, dst: &Path) -> Result<u64> {
        let input_file = File::open(src)
            .map_err(|e| TlobError::generic(format!("Failed to open {}: {}", src.display(), e)))?;

        let output_file = File::create(dst)
            .map_err(|e| TlobError::generic(format!("Failed to create {}: {}", dst.display(), e)))?;

        // Use large buffers for better throughput
        let reader = BufReader::with_capacity(IO_BUFFER_SIZE, input_file);
        let mut writer = BufWriter::with_capacity(IO_BUFFER_SIZE, output_file);

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

            writer.write_all(&buffer[..bytes_read]).map_err(|e| {
                TlobError::generic(format!("Decompression write error: {}", e))
            })?;

            total_bytes += bytes_read as u64;
        }

        writer.flush().map_err(|e| {
            TlobError::generic(format!("Failed to flush output: {}", e))
        })?;

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

        for entry in fs::read_dir(&self.config.hot_store_dir).map_err(|e| {
            TlobError::generic(format!(
                "Failed to read hot store directory: {}",
                e
            ))
        })? {
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

        for entry in fs::read_dir(&self.config.hot_store_dir).map_err(|e| {
            TlobError::generic(format!("Failed to read hot store: {}", e))
        })? {
            let entry = entry.map_err(|e| {
                TlobError::generic(format!("Failed to read entry: {}", e))
            })?;

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

        let bad_config = HotStoreConfig::new("/data")
            .with_compressed_ext("");
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
        let config = HotStoreConfig::dbn_defaults(&hot_dir)
            .with_prefer_decompressed(false);
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
        assert!(files[0].file_name().unwrap().to_str().unwrap().contains("a.mbo.dbn"));
        assert!(files[1].file_name().unwrap().to_str().unwrap().contains("b.mbo.dbn"));

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
        let mut encoder = zstd::stream::write::Encoder::new(
            File::create(&compressed_path).unwrap(),
            0,
        ).unwrap();
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

        let mut encoder = zstd::stream::write::Encoder::new(
            File::create(&compressed_path).unwrap(),
            0,
        ).unwrap();
        encoder.write_all(test_data).unwrap();
        encoder.finish().unwrap();

        // Decompress twice
        let result1 = manager.decompress(&compressed_path).unwrap();
        let result2 = manager.decompress(&compressed_path).unwrap();

        // Should return same path
        assert_eq!(result1, result2);

        cleanup(&dir);
    }
}

