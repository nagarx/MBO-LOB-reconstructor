//! Parquet writer for LOB snapshot data.
//!
//! Buffers [`LobState`] snapshots in a column-oriented batch, flushing to
//! Parquet row groups when the batch reaches the configured capacity.
//! Supports optional downsampling and derived-field computation.

use std::collections::HashMap;
use std::fs::File;
use std::path::Path;
use std::sync::Arc;

use parquet::arrow::ArrowWriter;
use parquet::basic::Compression;
use parquet::file::properties::WriterProperties;

use crate::error::{Result, TlobError};
use crate::types::LobState;

use super::batch::LobBatch;
use super::schema::lob_snapshot_schema;
use super::{DownsampleStats, DownsampleStrategy, ExportConfig, ParquetExportStats};

/// Internal decision returned by [`LobSnapshotWriter::write_decision`].
///
/// # Phase O Cycle 1 / B.3
///
/// Pre-B.3 the writer used `should_write(&self) -> bool` and could not
/// discriminate between "downsample-skip" (expected per config) and
/// "out-of-order-skip" (operator-actionable diagnostic per HFT-rules
/// §3 monotonic-timestamp invariant). The `WriteDecision` enum lets
/// `write_snapshot` route to the correct counter increment + log call,
/// without coupling the predicate to mutable state (per the F-021
/// structured-match precedent at `bin/export_to_parquet.rs:477-501`).
///
/// This enum is private; the public surface is [`ParquetExportStats`]
/// + [`DownsampleStats`].
enum WriteDecision {
    /// Write the snapshot to the batch.
    Write,
    /// Skip — the configured downsample strategy filtered the snapshot
    /// out (expected per config; not an anomaly).
    SkippedDownsample,
    /// Skip — out-of-order timestamp under
    /// [`DownsampleStrategy::MinIntervalNs`]. Pre-B.3 these were
    /// silently WRITTEN due to `(ts - last) as u64` casting negative
    /// `i64` to near-`u64::MAX` (always `>=` `min_ns`); post-B.3 the
    /// `i64::checked_sub` arm rejects + the writer increments
    /// `out_of_order_skipped`.
    SkippedOutOfOrder,
}

/// Writes [`LobState`] snapshots to a Parquet file.
///
/// # Usage
///
/// ```ignore
/// let config = ExportConfig::default();
/// let meta = HashMap::new();
/// let mut writer = LobSnapshotWriter::new("snapshots.parquet", &config, meta)?;
///
/// for state in lob_states {
///     writer.write_snapshot(&state)?;
/// }
///
/// let stats = writer.finish()?;
/// println!("Wrote {} rows", stats.rows_written);
/// ```
pub struct LobSnapshotWriter {
    writer: ArrowWriter<File>,
    schema: arrow::datatypes::Schema,
    batch: LobBatch,
    config: ExportConfig,

    // Downsample state
    rows_seen: u64,
    rows_written: u64,
    row_groups: u64,
    last_written_ts: Option<i64>,

    // Phase O Cycle 1 / B.3: per-writer counter for out-of-order timestamps
    // skipped under DownsampleStrategy::MinIntervalNs. Surfaced via
    // ParquetExportStats.downsample (Some(DownsampleStats { .. })) on
    // finish() when a downsample strategy is configured.
    out_of_order_skipped: u64,
}

impl LobSnapshotWriter {
    /// Create a new writer targeting `path`.
    ///
    /// `extra_metadata` is merged into the Parquet file-level metadata alongside
    /// the schema-level metadata (date, symbol, lob_levels, etc.).
    pub fn new(
        path: impl AsRef<Path>,
        config: &ExportConfig,
        extra_metadata: HashMap<String, String>,
    ) -> Result<Self> {
        config.validate()?;
        let levels = config.effective_levels();
        let mut schema = lob_snapshot_schema(levels, config.include_derived);

        let mut merged_meta = schema.metadata().clone();
        merged_meta.insert("lob_levels".into(), levels.to_string());
        for (k, v) in extra_metadata {
            merged_meta.insert(k, v);
        }
        schema = schema.with_metadata(merged_meta);

        let file = File::create(path.as_ref()).map_err(|e| {
            TlobError::Generic(format!(
                "Failed to create Parquet file '{}': {e}",
                path.as_ref().display()
            ))
        })?;

        let props = WriterProperties::builder()
            .set_compression(config.compression)
            .set_max_row_group_size(config.batch_size)
            .build();

        let writer = ArrowWriter::try_new(file, Arc::new(schema.clone()), Some(props))
            .map_err(|e| TlobError::Generic(format!("Failed to create ArrowWriter: {e}")))?;

        let batch = LobBatch::new(config.batch_size, levels, config.include_derived);

        Ok(Self {
            writer,
            schema,
            batch,
            config: config.clone(),
            rows_seen: 0,
            rows_written: 0,
            row_groups: 0,
            last_written_ts: None,
            out_of_order_skipped: 0,
        })
    }

    /// Write a single LOB snapshot.
    ///
    /// The snapshot may be skipped if it does not pass the downsampling
    /// filter. Phase O Cycle 1 / B.3 (post-cycle): the routing now
    /// discriminates between expected downsample-skip (silent) and
    /// out-of-order-skip (counter increment + WARN log).
    pub fn write_snapshot(&mut self, state: &LobState) -> Result<()> {
        self.rows_seen += 1;

        match self.write_decision(state) {
            WriteDecision::Write => {
                self.batch.push(state);
                self.rows_written += 1;
                // Phase O Cycle 1 / B.3 (C-4 latent fix): only update the
                // throttle anchor when the written snapshot has an explicit
                // timestamp. Pre-B.3, a None-timestamp snapshot in the
                // middle of a stream would OVERWRITE a `Some(last)` anchor
                // with `None`, resetting the throttle so the NEXT snapshot
                // was always written regardless of its delta (silent
                // downsample-leakage). Post-B.3 the anchor only advances
                // when we have a real timestamp to anchor against.
                if state.timestamp.is_some() {
                    self.last_written_ts = state.timestamp;
                }

                if self.batch.is_full() {
                    self.flush_batch()?;
                }
            }
            WriteDecision::SkippedDownsample => {
                // Expected per config — no observability needed.
            }
            WriteDecision::SkippedOutOfOrder => {
                // Phase O Cycle 1 / B.3: fail-loud counter for upstream
                // monotonic-timestamp invariant violations (HFT-rules §3
                // + §8 — never silently drop without recording diagnostics).
                self.out_of_order_skipped += 1;
                log::warn!(
                    "LobSnapshotWriter: snapshot skipped due to out-of-order \
                     timestamp under DownsampleStrategy::MinIntervalNs \
                     (out_of_order_skipped now = {}); upstream timestamp \
                     issue in LobReconstructor's input stream — operators \
                     should audit the source data per HFT-rules §3.",
                    self.out_of_order_skipped,
                );
            }
        }
        Ok(())
    }

    /// Flush remaining buffered rows and close the Parquet file.
    ///
    /// Returns export statistics. This **must** be called to produce a valid
    /// Parquet file (the footer is written here).
    ///
    /// Phase O Cycle 1 / B.3: when the writer was configured with a
    /// downsample strategy (`config.downsample.is_some()`), the returned
    /// stats include `Some(DownsampleStats { out_of_order_skipped, .. })`
    /// surfacing the per-writer out-of-order counter for operator
    /// observability. When no downsample strategy was configured, the
    /// `downsample` field is `None`.
    pub fn finish(mut self) -> Result<ParquetExportStats> {
        if self.batch.len() > 0 {
            self.flush_batch()?;
        }

        self.writer
            .close()
            .map_err(|e| TlobError::Generic(format!("Failed to close Parquet writer: {e}")))?;

        let downsample = self.config.downsample.as_ref().map(|_| DownsampleStats {
            out_of_order_skipped: self.out_of_order_skipped,
        });

        Ok(ParquetExportStats {
            rows_written: self.rows_written,
            rows_seen: self.rows_seen,
            row_groups: self.row_groups,
            downsample,
        })
    }

    /// Number of rows written so far (after downsampling).
    pub fn rows_written(&self) -> u64 {
        self.rows_written
    }

    /// Number of rows seen so far (before downsampling).
    pub fn rows_seen(&self) -> u64 {
        self.rows_seen
    }

    fn flush_batch(&mut self) -> Result<()> {
        let record_batch = self.batch.flush(&self.schema)?;
        self.writer
            .write(&record_batch)
            .map_err(|e| TlobError::Generic(format!("Failed to write row group: {e}")))?;
        self.row_groups += 1;
        Ok(())
    }

    /// Decide whether to write the given snapshot, and if not, why
    /// (Phase O Cycle 1 / B.3 enum return).
    ///
    /// Pure function (`&self`) — counter mutation lives in the caller
    /// (`write_snapshot`'s match arm). This separation matches the
    /// F-021 structured-match precedent at
    /// `bin/export_to_parquet.rs:477-501`.
    ///
    /// # Phase O Cycle 1 / B.3 — wraparound bug closure
    ///
    /// The `MinIntervalNs` arm now uses `i64::checked_sub` to detect
    /// non-monotonic timestamps (`ts < last`) and routes them to
    /// [`WriteDecision::SkippedOutOfOrder`] instead of falling through
    /// the pre-B.3 buggy expression `(ts - last) as u64 >= *min_ns`,
    /// which silently flipped to `~u64::MAX >= min_ns == TRUE` on any
    /// negative `i64` subtraction (always wrote, never skipped — silent
    /// semantic inversion under jittery feeds with documented late-
    /// arrival message-delivery patterns; see
    /// `MBO-LOB-reconstructor/WARNINGS.md` § ORDER_NOT_FOUND for the
    /// upstream context).
    ///
    /// `MinIntervalNs(0)` is a LEGITIMATE post-B.3 use case: it acts
    /// as a "monotonic-only filter" — writes every snapshot whose
    /// timestamp is `>= last_written_ts`, skips every out-of-order
    /// snapshot. Pre-B.3 the expression silently degenerated to "write
    /// every snapshot" (because `(negative) as u64 >= 0` is always
    /// true); the post-B.3 semantics are well-defined and operators
    /// can rely on them.
    fn write_decision(&self, state: &LobState) -> WriteDecision {
        let strategy = match &self.config.downsample {
            Some(ds) => &ds.strategy,
            None => return WriteDecision::Write,
        };

        match strategy {
            DownsampleStrategy::None => WriteDecision::Write,
            DownsampleStrategy::EveryN(n) => {
                let n = *n;
                if n == 0 {
                    return WriteDecision::Write;
                }
                if (self.rows_seen - 1) % (n as u64) == 0 {
                    WriteDecision::Write
                } else {
                    WriteDecision::SkippedDownsample
                }
            }
            DownsampleStrategy::MinIntervalNs(min_ns) => {
                let ts = match state.timestamp {
                    Some(t) => t,
                    None => return WriteDecision::Write,
                };
                match self.last_written_ts {
                    None => WriteDecision::Write,
                    Some(last) => {
                        // Phase O Cycle 1 / B.3 wraparound closure:
                        // pre-B.3 used `(ts - last) as u64 >= *min_ns`
                        // which silently flipped to `~u64::MAX` on
                        // out-of-order timestamps (ts < last) → always
                        // >= min_ns → snapshot ALWAYS WRITTEN. Now:
                        // checked_sub on i64 + skip on negative.
                        match ts.checked_sub(last) {
                            Some(delta) if delta >= 0 => {
                                if (delta as u64) >= *min_ns {
                                    WriteDecision::Write
                                } else {
                                    WriteDecision::SkippedDownsample
                                }
                            }
                            _ => WriteDecision::SkippedOutOfOrder,
                        }
                    }
                }
            }
        }
    }
}

/// Builder for [`LobSnapshotWriter`] with a fluent API.
///
/// ```ignore
/// let writer = LobSnapshotWriter::builder("output.parquet")
///     .levels(10)
///     .include_derived(true)
///     .compression(Compression::SNAPPY)
///     .date("2025-02-03")
///     .symbol("NVDA")
///     .build()?;
/// ```
pub struct LobSnapshotWriterBuilder {
    path: std::path::PathBuf,
    config: ExportConfig,
    metadata: HashMap<String, String>,
}

impl LobSnapshotWriterBuilder {
    pub fn new(path: impl AsRef<Path>) -> Self {
        Self {
            path: path.as_ref().to_path_buf(),
            config: ExportConfig::default(),
            metadata: HashMap::new(),
        }
    }

    pub fn levels(mut self, levels: usize) -> Self {
        self.config.levels = levels;
        self
    }

    pub fn include_derived(mut self, include: bool) -> Self {
        self.config.include_derived = include;
        self
    }

    pub fn batch_size(mut self, size: usize) -> Self {
        self.config.batch_size = size;
        self
    }

    pub fn compression(mut self, compression: Compression) -> Self {
        self.config.compression = compression;
        self
    }

    pub fn date(mut self, date: &str) -> Self {
        self.metadata.insert("date".into(), date.into());
        self
    }

    pub fn symbol(mut self, symbol: &str) -> Self {
        self.metadata.insert("symbol".into(), symbol.into());
        self
    }

    pub fn metadata(mut self, key: &str, value: &str) -> Self {
        self.metadata.insert(key.into(), value.into());
        self
    }

    pub fn build(self) -> Result<LobSnapshotWriter> {
        LobSnapshotWriter::new(self.path, &self.config, self.metadata)
    }
}

impl LobSnapshotWriter {
    pub fn builder(path: impl AsRef<Path>) -> LobSnapshotWriterBuilder {
        LobSnapshotWriterBuilder::new(path)
    }
}
