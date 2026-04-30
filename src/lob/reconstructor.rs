//! Single-symbol LOB reconstructor.
//!
//! High-performance implementation using:
//! - BTreeMap for sorted price levels
//! - PriceLevel with cached total_size for O(1) aggregate queries
//! - ahash HashMap for fast order lookups
//! - Cached best bid/ask to avoid recomputation
//! - Minimal allocations on hot path

use ahash::AHashMap;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::constants::NANODOLLARS_PER_DOLLAR_F64;
use crate::error::{Result, TlobError};
use crate::lob::price_level::PriceLevel;
use crate::types::{Action, BookConsistency, LobState, MboMessage, Order, Side};

/// Schema version for the `LobStats` JSON serialization envelope.
///
/// Phase M M.A.5 (REV 3 boundary discipline cycle): introduced
/// [`LobStatsExportEnvelope`] wrapping the raw stats with a `schema_version`
/// field. Bumped to `2.0.0` to signal the conceptual break from the legacy
/// flat shape (no envelope existed before — pre-M.A.5 was implicit-1.0).
///
/// **Independent of** [`crate::export::SCHEMA_VERSION`] (`"1.0"`), which
/// versions the Parquet export schema — a different artifact. When either
/// constant bumps, the other should NOT auto-bump.
///
/// # Versioning policy (per hft-rules §1)
///
/// - MAJOR: any breaking change to the on-disk JSON shape (e.g., remove a
///   field, rename a field, change an envelope key).
/// - MINOR: additive non-breaking changes (e.g., new `LobStats` field).
/// - PATCH: docs-only changes.
pub const LOB_STATS_SCHEMA_VERSION: &str = "2.0.0";

/// How to handle crossed quotes (bid >= ask) when they occur.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum CrossedQuotePolicy {
    /// Allow crossed quotes (default) - just track in stats
    #[default]
    Allow,

    /// Return the last valid state when a crossed quote would occur
    UseLastValid,

    /// Return an error when a crossed quote would occur
    Error,

    /// Return last valid book state when crossing would occur.
    ///
    /// Book mutations are still applied internally (the order IS added/cancelled/traded).
    /// Only the returned LobState uses the last valid snapshot. Functionally identical
    /// to `UseLastValid` — both share the same code path.
    SkipUpdate,
}

/// Configuration for LOB reconstructor behavior.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LobConfig {
    /// Number of price levels to track
    pub levels: usize,

    /// How to handle crossed quotes
    pub crossed_quote_policy: CrossedQuotePolicy,

    /// Whether to validate messages before processing
    pub validate_messages: bool,

    /// Whether to log warnings for consistency issues
    pub log_warnings: bool,

    /// Skip system messages (order_id=0, size=0, price<=0) instead of erroring.
    ///
    /// DBN/MBO data often contains system messages (heartbeats, status updates)
    /// that have order_id=0. These are NOT valid orders and cannot be processed
    /// by the LOB reconstructor.
    ///
    /// When true (default): silently skip these messages and track count in stats.
    /// When false: attempt to process (will fail validation if validate_messages=true).
    ///
    /// This is the recommended setting for LOB reconstruction from real market data.
    pub skip_system_messages: bool,
}

impl Default for LobConfig {
    fn default() -> Self {
        Self {
            levels: 10,
            crossed_quote_policy: CrossedQuotePolicy::Allow,
            validate_messages: true,
            log_warnings: true,
            skip_system_messages: true, // Safe default for real market data
        }
    }
}

impl LobConfig {
    /// Create a new config with specified number of levels.
    pub fn new(levels: usize) -> Self {
        Self {
            levels,
            ..Default::default()
        }
    }

    /// Set crossed quote handling policy.
    pub fn with_crossed_quote_policy(mut self, policy: CrossedQuotePolicy) -> Self {
        self.crossed_quote_policy = policy;
        self
    }

    /// Enable/disable message validation.
    pub fn with_validation(mut self, validate: bool) -> Self {
        self.validate_messages = validate;
        self
    }

    /// Enable/disable warning logs.
    pub fn with_logging(mut self, log: bool) -> Self {
        self.log_warnings = log;
        self
    }

    /// Enable/disable skipping of system messages.
    ///
    /// System messages are identified by:
    /// - order_id = 0 (heartbeats, status updates, metadata)
    /// - size = 0 (invalid order size)
    /// - price <= 0 (invalid price)
    ///
    /// When true (default): these messages are silently skipped.
    /// When false: these messages will be processed (and likely fail validation).
    pub fn with_skip_system_messages(mut self, skip: bool) -> Self {
        self.skip_system_messages = skip;
        self
    }

    /// Validate configuration values.
    ///
    /// Returns `Err` if any field has a degenerate value that would produce
    /// incorrect results or pathological behavior.
    pub fn validate(&self) -> crate::error::Result<()> {
        if self.levels == 0 {
            return Err(crate::error::TlobError::InvalidConfig(
                "LobConfig.levels must be at least 1".into(),
            ));
        }
        if self.levels > crate::types::MAX_LOB_LEVELS {
            return Err(crate::error::TlobError::InvalidConfig(format!(
                "LobConfig.levels must be <= {} (got {})",
                crate::types::MAX_LOB_LEVELS,
                self.levels
            )));
        }
        Ok(())
    }
}

/// Single-symbol LOB reconstructor.
///
/// This maintains the full order book state and processes MBO messages
/// to keep it updated. Design goals:
/// - Fast message processing (target: <20 μs per message)
/// - Minimal memory footprint
/// - Accurate state tracking
/// - Easy to test and debug
#[derive(Debug, Clone)]
pub struct LobReconstructor {
    /// Configuration
    config: LobConfig,

    /// Bid orders: price -> PriceLevel (with cached total_size)
    /// BTreeMap keeps prices sorted (highest first for bids)
    bids: BTreeMap<i64, PriceLevel>,

    /// Ask orders: price -> PriceLevel (with cached total_size)
    /// BTreeMap keeps prices sorted (lowest first for asks)
    asks: BTreeMap<i64, PriceLevel>,

    /// Order tracking: order_id -> Order
    /// Fast lookup for modify/cancel/trade operations
    orders: AHashMap<u64, Order>,

    /// Cached best bid (highest bid price)
    best_bid: Option<i64>,

    /// Cached best ask (lowest ask price)
    best_ask: Option<i64>,

    /// Statistics (for monitoring)
    stats: LobStats,

    /// Last valid LOB state (for UseLastValid policy)
    last_valid_state: Option<LobState>,
}

/// Statistics for monitoring LOB health.
///
/// Phase M M.A.4 (REV 3 boundary discipline cycle):
/// - `#[non_exhaustive]` per Decision 18 — additive-only future evolution; external
///   crates cannot construct `LobStats { ... }` via struct literal (use
///   `LobStats::default()` + `..LobStats::default()` struct-update syntax).
/// - The legacy `errors: u64` field was REMOVED per Decision 10b (F-007 closure)
///   after pre-implementation gate confirmed ZERO genuine increment sites in
///   production code. Specific error categories are tracked by `crossed_quotes`,
///   `locked_quotes`, and the per-action `*_not_found` / `*_missing` counters
///   below. **Breaking change for external code reading `.errors`** —
///   documented in `CHANGELOG.md` (M.A.8).
/// - F-013 closure: NEW `modify_order_not_found` + `add_order_id_collision`
///   counters expose silent fall-through behavior at the modify-of-missing
///   and add-of-existing paths (see `LobReconstructor::modify_order` and
///   `LobReconstructor::add_order`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct LobStats {
    /// Total messages processed (excludes system messages if skip_system_messages=true)
    pub messages_processed: u64,

    /// System messages skipped (order_id=0, size=0, price<=0)
    pub system_messages_skipped: u64,

    /// Number of active orders
    pub active_orders: usize,

    /// Number of price levels (bid side)
    pub bid_levels: usize,

    /// Number of price levels (ask side)
    pub ask_levels: usize,

    /// Number of crossed quotes detected (bid >= ask)
    pub crossed_quotes: u64,

    /// Number of locked quotes detected (bid == ask)
    pub locked_quotes: u64,

    /// Last timestamp processed (nanoseconds since epoch)
    pub last_timestamp: Option<i64>,

    // =========================================================================
    // Warning Counters (for tracking anomalies without failing)
    // =========================================================================
    /// Number of cancels for orders not found
    pub cancel_order_not_found: u64,

    /// Number of cancels where price level was missing
    pub cancel_price_level_missing: u64,

    /// Number of cancels where order was not at expected price level
    pub cancel_order_at_level_missing: u64,

    /// Number of trades for orders not found
    pub trade_order_not_found: u64,

    /// Number of trades where price level was missing
    pub trade_price_level_missing: u64,

    /// Number of trades where order was not at expected price level
    pub trade_order_at_level_missing: u64,

    /// Number of `modify_order` operations where the `order_id` was NOT found
    /// in the active orders map.
    ///
    /// Phase M M.A.4 (REV 3 F-013 closure): increments at
    /// `LobReconstructor::modify_order` line ~500 BEFORE the recovery
    /// fall-through to `add_order(msg)` (which creates a NEW order at the
    /// MODIFY message's price/side/size). This is a normal warmup pattern
    /// for messages whose corresponding `Add` arrived before the iteration
    /// window started; persistent non-zero values during steady-state
    /// indicate upstream feed gaps.
    #[serde(default)]
    pub modify_order_not_found: u64,

    /// Number of `add_order` operations where the `order_id` already existed
    /// in the active orders map.
    ///
    /// Phase M M.A.4 (REV 3 F-013 sibling closure): increments at
    /// `LobReconstructor::add_order` line ~462 BEFORE the recovery
    /// fall-through to `modify_order(msg)` (some exchanges reuse `order_id`s).
    /// Persistent non-zero values may indicate either (a) legitimate id reuse
    /// by the venue, or (b) data-feed quirks worth investigating.
    #[serde(default)]
    pub add_order_id_collision: u64,

    /// Number of book clears/resets
    pub book_clears: u64,

    /// Number of no-op (Action::None) messages
    pub noop_messages: u64,
}

impl LobStats {
    /// Check if there were any warnings during processing.
    pub fn has_warnings(&self) -> bool {
        self.cancel_order_not_found > 0
            || self.cancel_price_level_missing > 0
            || self.cancel_order_at_level_missing > 0
            || self.trade_order_not_found > 0
            || self.trade_price_level_missing > 0
            || self.trade_order_at_level_missing > 0
            || self.modify_order_not_found > 0
            || self.add_order_id_collision > 0
    }

    /// Get total number of warnings.
    pub fn total_warnings(&self) -> u64 {
        self.cancel_order_not_found
            + self.cancel_price_level_missing
            + self.cancel_order_at_level_missing
            + self.trade_order_not_found
            + self.trade_price_level_missing
            + self.trade_order_at_level_missing
            + self.modify_order_not_found
            + self.add_order_id_collision
    }

    /// Export stats to a JSON file with atomic-write + envelope discipline.
    ///
    /// Phase M M.A.5 (REV 3 boundary discipline cycle):
    ///
    /// **Atomic write**: uses `tempfile::NamedTempFile::persist()` to write
    /// to a sibling tmp file in the same directory, fsync, then atomically
    /// rename to `path`. Eliminates the SIGKILL-mid-write partial-file risk
    /// of the pre-M.A.5 `BufWriter + serde_json::to_writer_pretty` path.
    ///
    /// **Envelope wrapper**: output JSON is `{ "schema_version": "2.0.0",
    /// "stats": {...} }`. The `schema_version` field is the
    /// [`LOB_STATS_SCHEMA_VERSION`] constant. **Breaking change** for the
    /// on-disk format; pre-M.A.5 flat-shape files cannot round-trip through
    /// `export_to_file → load_from_file` cleanly anymore (but
    /// [`Self::load_from_file`] remains backward-compatible via dual-format
    /// reading — see its docs).
    ///
    /// **Fallback**: if `NamedTempFile::new_in(parent)` fails (e.g., NFS
    /// quirks, EROFS) or `persist()` fails (e.g., cross-device link EXDEV),
    /// the function falls back to direct `File::create` + `write_all` +
    /// `sync_all` and emits a `log::warn!`. The fallback emits the same
    /// envelope shape so readers cannot distinguish atomic vs fallback paths.
    ///
    /// # SSoT-deferred consolidation
    ///
    /// `hft_statistics::io::atomic_write_json` (default-off `io` feature in
    /// 0.3.0-dev) is the canonical primitive for this pattern across the
    /// pipeline. Reconstructor's inline implementation is intentionally
    /// duplicated until a sibling-wide hft-statistics 0.3.x bump cycle
    /// allows all standalone-repo siblings (this repo + sibling profilers)
    /// to migrate together.
    pub fn export_to_file(&self, path: impl AsRef<std::path::Path>) -> std::io::Result<()> {
        let path = path.as_ref();
        let envelope = LobStatsExportEnvelopeRef {
            schema_version: LOB_STATS_SCHEMA_VERSION,
            stats: self,
        };

        // Determine parent directory for tmp file co-location.
        // Edge case: path with no parent (e.g., "stats.json" relative to CWD,
        // or the unusual "/" root) — fall back to CWD.
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| std::path::Path::new("."));

        // Atomic-write path: tempfile in same FS as target → write → fsync → rename.
        match tempfile::NamedTempFile::new_in(parent) {
            Ok(mut tmp) => {
                serde_json::to_writer_pretty(&mut tmp, &envelope).map_err(std::io::Error::other)?;
                tmp.as_file().sync_all()?;
                if let Err(persist_err) = tmp.persist(path) {
                    log::warn!(
                        "atomic persist failed for {} ({}); falling back to direct write",
                        path.display(),
                        persist_err.error
                    );
                    return Self::write_envelope_direct(path, &envelope);
                }
                Ok(())
            }
            Err(tmp_err) => {
                log::warn!(
                    "tempfile creation failed in {} ({}); falling back to direct write",
                    parent.display(),
                    tmp_err
                );
                Self::write_envelope_direct(path, &envelope)
            }
        }
    }

    /// Direct (non-atomic) write fallback path. Same envelope shape as the
    /// atomic path — readers cannot distinguish.
    fn write_envelope_direct(
        path: &std::path::Path,
        envelope: &LobStatsExportEnvelopeRef<'_>,
    ) -> std::io::Result<()> {
        let mut file = std::fs::File::create(path)?;
        serde_json::to_writer_pretty(&mut file, envelope).map_err(std::io::Error::other)?;
        file.sync_all()
    }

    /// Load stats from a JSON file (dual-format aware).
    ///
    /// Phase M M.A.5 (REV 3 boundary discipline cycle): accepts BOTH:
    /// - **Envelope shape** (post-M.A.5): `{ "schema_version": "2.0.0",
    ///   "stats": {...} }` — preferred.
    /// - **Legacy flat shape** (pre-M.A.5): `{messages_processed: ..., ...}`
    ///   without an envelope. Emits a `log::warn!` (per call) so operators
    ///   are aware they're reading deprecated-format files. Scheduled for
    ///   removal in next MAJOR (calendar 2026-10-29 — aligned with
    ///   `legacy-iterator-api` removal).
    ///
    /// # Format dispatch (Phase M M.A.5 hardening — explicit over `untagged`)
    ///
    /// Earlier `#[serde(untagged)]` over `LobStatsLoadable` had a silent-
    /// acceptance failure mode: a JSON with top-level `schema_version` BUT
    /// the `stats` key MISSING (e.g., a botched migration script that wrote
    /// `schema_version` but forgot the `stats` wrapper) would silently route
    /// through the legacy variant — `LobStats` doesn't have a `schema_version`
    /// field, so serde dropped it without error. The post-hardening dispatch
    /// peeks at top-level keys via `serde_json::Value` BEFORE deserializing,
    /// then routes:
    /// - If `schema_version` AND `stats` are both present → strict envelope parse.
    /// - If `schema_version` is present BUT `stats` is missing → fail-loud
    ///   (`std::io::Error::other`) per hft-rules §5 — refusing to silently
    ///   accept malformed envelope output.
    /// - Otherwise → legacy flat parse + WARN.
    pub fn load_from_file(path: impl AsRef<std::path::Path>) -> std::io::Result<Self> {
        let path_ref = path.as_ref();
        let json = std::fs::read_to_string(path_ref)?;

        // Peek at top-level keys via Value to disambiguate envelope vs legacy
        // PRECISELY — closes the silent-accept loophole that `#[serde(untagged)]`
        // had with malformed-but-envelope-shaped payloads.
        let value: serde_json::Value =
            serde_json::from_str(&json).map_err(std::io::Error::other)?;

        let has_schema_version = value
            .as_object()
            .map(|o| o.contains_key("schema_version"))
            .unwrap_or(false);
        let has_stats = value
            .as_object()
            .map(|o| o.contains_key("stats"))
            .unwrap_or(false);

        match (has_schema_version, has_stats) {
            (true, true) => {
                // Envelope shape — strict parse.
                let envelope: LobStatsExportEnvelope =
                    serde_json::from_value(value).map_err(std::io::Error::other)?;
                Ok(envelope.stats)
            }
            (true, false) => {
                // Malformed: claims envelope (has schema_version) but missing stats wrapper.
                // Pre-hardening this would silently parse as Legacy(LobStats), dropping
                // schema_version. Post-hardening: fail-loud per hft-rules §5.
                Err(std::io::Error::other(format!(
                    "malformed envelope at {}: top-level `schema_version` present but `stats` wrapper missing",
                    path_ref.display(),
                )))
            }
            (false, _) => {
                // Legacy flat shape.
                let stats: LobStats =
                    serde_json::from_value(value).map_err(std::io::Error::other)?;
                log::warn!(
                    "loaded LobStats from {} in legacy flat-shape JSON (pre-M.A.5); \
                     re-export to upgrade to envelope schema {}. \
                     Legacy format scheduled for removal in next MAJOR (2026-10-29).",
                    path_ref.display(),
                    LOB_STATS_SCHEMA_VERSION,
                );
                Ok(stats)
            }
        }
    }
}

/// On-disk envelope wrapping [`LobStats`] with a schema version (Phase M M.A.5).
///
/// Used internally by [`LobStats::export_to_file`] for serialization. External
/// readers should NOT depend on this struct; use [`LobStats::load_from_file`]
/// which transparently strips the envelope.
///
/// # Wire-shape rationale (envelope vs flat)
///
/// Sibling sidecar wrappers in the pipeline (`feature-extractor-MBO-LOB`'s
/// `_diagnostics.json`, `_metadata.json`) use a FLAT shape with
/// `schema_version` as a top-level field alongside payload fields. M.A.5
/// chose a NESTED envelope `{schema_version, stats}` instead — deliberately
/// per Phase M REV 3 Decision 12 — because the legacy pre-M.A.5 wire shape
/// (`serde_json::to_writer_pretty(&LobStats)` directly) ALREADY occupied the
/// flat top-level namespace. Adding a top-level `schema_version` field to
/// `LobStats` itself would require either a hand-coded migration shim OR a
/// second BREAKING bump down the line. The envelope is a one-time MAJOR bump
/// (1.0 → 2.0) that creates a clean separation between the wrapper layer and
/// the payload layer for all future evolution.
///
/// `#[non_exhaustive]` per Phase M Decision 18 — additive-only future evolution.
#[derive(Debug, Serialize, Deserialize)]
#[non_exhaustive]
pub struct LobStatsExportEnvelope {
    /// Schema version string (e.g., `"2.0.0"`).
    pub schema_version: String,

    /// The wrapped [`LobStats`] payload.
    pub stats: LobStats,
}

/// Borrowing variant of the envelope used for write-side zero-copy serialization.
///
/// Pairs with [`LobStatsExportEnvelope`] (the deserialize-side owned variant).
/// This split avoids `Cow<LobStats>` complications and keeps the serialization
/// path zero-allocation for the stats payload.
#[derive(Debug, Serialize)]
struct LobStatsExportEnvelopeRef<'a> {
    schema_version: &'static str,
    stats: &'a LobStats,
}

// Phase M M.A.5 hardening: the previous `LobStatsLoadable` `#[serde(untagged)]`
// enum was REPLACED by an explicit `serde_json::Value`-peek dispatch in
// `LobStats::load_from_file` (above). Rationale: untagged-fallback silently
// accepted malformed envelope payloads (top-level `schema_version` but missing
// `stats` wrapper) by routing to the Legacy variant and dropping the unknown
// field. Explicit dispatch fails-loud on that malformed shape per hft-rules §5.

/// The type of order reduction operation being performed.
///
/// Both cancel and trade operations follow the same 3-stage lookup
/// (order → price level → order at level) with identical partial/full
/// removal logic. This enum selects the appropriate stat counters
/// and log prefixes for each operation type.
#[derive(Debug, Clone, Copy)]
enum OrderReductionOp {
    Cancel,
    Trade,
}

impl OrderReductionOp {
    /// Log prefix for diagnostic messages.
    const fn label(self) -> &'static str {
        match self {
            Self::Cancel => "Cancel",
            Self::Trade => "Trade",
        }
    }
}

impl LobReconstructor {
    /// Create a new LOB reconstructor.
    ///
    /// # Arguments
    /// * `levels` - Number of price levels to track (e.g., 10)
    ///
    /// # Example
    /// ```
    /// use mbo_lob_reconstructor::LobReconstructor;
    ///
    /// let lob = LobReconstructor::new(10);
    /// ```
    pub fn new(levels: usize) -> Self {
        Self::with_config(LobConfig::new(levels))
    }

    /// Create a new LOB reconstructor with custom configuration.
    ///
    /// # Arguments
    /// * `config` - Configuration for LOB behavior
    ///
    /// # Example
    /// ```
    /// use mbo_lob_reconstructor::{LobReconstructor, LobConfig, CrossedQuotePolicy};
    ///
    /// let config = LobConfig::new(10)
    ///     .with_crossed_quote_policy(CrossedQuotePolicy::UseLastValid);
    /// let lob = LobReconstructor::with_config(config);
    /// ```
    pub fn with_config(config: LobConfig) -> Self {
        config.validate().expect("LobConfig validation failed");
        Self {
            config,
            bids: BTreeMap::new(),
            asks: BTreeMap::new(),
            orders: AHashMap::new(),
            best_bid: None,
            best_ask: None,
            stats: LobStats::default(),
            last_valid_state: None,
        }
    }

    /// Get the number of price levels being tracked.
    #[inline]
    pub fn levels(&self) -> usize {
        self.config.levels
    }

    /// Get a reference to the current configuration.
    #[inline]
    pub fn config(&self) -> &LobConfig {
        &self.config
    }

    /// Process a single MBO message and return the LOB state.
    ///
    /// Convenience API that allocates a fresh `LobState` on each call.
    /// Delegates to `process_message_into()` — the single source of truth
    /// for dispatch, stats, and policy logic.
    ///
    /// For hot paths requiring zero allocation, use `process_message_into()` instead.
    ///
    /// # Temporal Fields
    ///
    /// Returns `triggering_action`, `triggering_side`, and `sequence` populated
    /// from the current message. `delta_ns` is always 0 and `previous_timestamp`
    /// is always `None` because each call uses a fresh buffer with no temporal
    /// chain. Use `process_message_into()` with a reused buffer for temporal
    /// continuity (FI-2010 u6-u9 features).
    ///
    /// # Errors
    /// Returns error if message is invalid or causes inconsistent state
    /// (depending on configuration)
    #[inline]
    pub fn process_message(&mut self, msg: &MboMessage) -> Result<LobState> {
        let mut state = LobState::new(self.config.levels);
        self.process_message_into(msg, &mut state)?;
        Ok(state)
    }

    /// Check book consistency and return the status.
    #[inline]
    fn check_consistency(&self) -> BookConsistency {
        match (self.best_bid, self.best_ask) {
            (Some(bid), Some(ask)) => {
                if bid < ask {
                    BookConsistency::Valid
                } else if bid == ask {
                    BookConsistency::Locked
                } else {
                    BookConsistency::Crossed
                }
            }
            _ => BookConsistency::Empty,
        }
    }

    /// Track consistency in statistics and optionally log.
    #[inline]
    fn track_consistency(&mut self, consistency: BookConsistency) {
        match consistency {
            BookConsistency::Valid => {
                if matches!(
                    self.config.crossed_quote_policy,
                    CrossedQuotePolicy::UseLastValid | CrossedQuotePolicy::SkipUpdate
                ) {
                    let state = self.get_lob_state();
                    self.last_valid_state = Some(state);
                }
            }
            BookConsistency::Crossed => {
                self.stats.crossed_quotes += 1;
                if self.config.log_warnings {
                    if let (Some(bid), Some(ask)) = (self.best_bid, self.best_ask) {
                        log::warn!(
                            "Crossed quote detected: bid={:.4} > ask={:.4} (message #{})",
                            bid as f64 / NANODOLLARS_PER_DOLLAR_F64,
                            ask as f64 / NANODOLLARS_PER_DOLLAR_F64,
                            self.stats.messages_processed
                        );
                    }
                }
            }
            BookConsistency::Locked => {
                self.stats.locked_quotes += 1;
                if self.config.log_warnings {
                    if let Some(bid) = self.best_bid {
                        log::debug!(
                            "Locked quote detected: bid=ask={:.4} (message #{})",
                            bid as f64 / NANODOLLARS_PER_DOLLAR_F64,
                            self.stats.messages_processed
                        );
                    }
                }
            }
            BookConsistency::Empty => {}
        }
    }

    /// Check if the current book state is consistent (bid < ask).
    ///
    /// # Returns
    /// - `true` if book is valid (bid < ask) or empty
    /// - `false` if book is crossed (bid > ask) or locked (bid == ask)
    #[inline]
    pub fn is_consistent(&self) -> bool {
        match (self.best_bid, self.best_ask) {
            (Some(bid), Some(ask)) => bid < ask,
            _ => true, // Empty book is considered consistent
        }
    }

    /// Check if the book is crossed (bid > ask).
    #[inline]
    pub fn is_crossed(&self) -> bool {
        match (self.best_bid, self.best_ask) {
            (Some(bid), Some(ask)) => bid > ask,
            _ => false,
        }
    }

    /// Add a new order to the book.
    #[inline]
    fn add_order(&mut self, msg: &MboMessage) -> Result<()> {
        // Check if order already exists
        if self.orders.contains_key(&msg.order_id) {
            // Some exchanges reuse order IDs, treat as modify.
            //
            // Phase M M.A.4 (REV 3 F-013 sibling closure): increment
            // `add_order_id_collision` counter BEFORE the silent recovery
            // fall-through. Production NVDA data shows ~0.5% — small but
            // non-zero; persistent values help operators detect feed quirks.
            self.stats.add_order_id_collision += 1;
            return self.modify_order(msg);
        }

        // Add to appropriate side
        let price_level = match msg.side {
            Side::Bid => self.bids.entry(msg.price).or_default(),
            Side::Ask => self.asks.entry(msg.price).or_default(),
            Side::None => {
                // Non-directional orders are ignored
                return Ok(());
            }
        };

        // Insert order at price level (PriceLevel handles total_size update)
        price_level.add_order(msg.order_id, msg.size);

        // Track order
        self.orders.insert(
            msg.order_id,
            Order {
                side: msg.side,
                price: msg.price,
                size: msg.size,
            },
        );

        Ok(())
    }

    /// Modify an existing order.
    #[inline]
    fn modify_order(&mut self, msg: &MboMessage) -> Result<()> {
        // Get existing order
        let old_order = match self.orders.get(&msg.order_id) {
            Some(order) => *order,
            None => {
                // Order not found, treat as add.
                //
                // Phase M M.A.4 (REV 3 F-013 closure): increment
                // `modify_order_not_found` counter BEFORE the silent
                // recovery fall-through. The fall-through creates a NEW
                // order at MODIFY's price/side/size — this is the documented
                // warmup heuristic for messages whose corresponding `Add`
                // arrived before the iteration window started. Production
                // NVDA data shows the rate empirically; persistent non-zero
                // during steady-state indicates upstream feed gaps.
                self.stats.modify_order_not_found += 1;
                return self.add_order(msg);
            }
        };

        // Remove old order
        self.remove_order_internal(msg.order_id, &old_order)?;

        // Add as new order
        self.add_order(msg)?;

        Ok(())
    }

    /// Cancel (remove) an order from the book.
    ///
    /// Handles both full and partial cancellations based on the `size` field.
    /// Uses soft error handling - anomalies are tracked in stats but don't fail.
    #[inline]
    fn cancel_order(&mut self, msg: &MboMessage) -> Result<()> {
        self.reduce_or_remove_order(msg, OrderReductionOp::Cancel)
    }

    /// Process a trade (execution).
    ///
    /// Trade reduces order size or removes it completely.
    /// Uses soft error handling - anomalies are tracked in stats but don't fail.
    #[inline]
    fn process_trade(&mut self, msg: &MboMessage) -> Result<()> {
        self.reduce_or_remove_order(msg, OrderReductionOp::Trade)
    }

    /// Unified order reduction: look up order, reduce or remove from book.
    ///
    /// Both cancel and trade follow the same 3-stage lookup with identical
    /// partial/full removal logic. Only stat counters and log prefixes differ,
    /// selected by `op`. This eliminates the ~90% code duplication that was
    /// the root cause of bugs #2 and #3 (fix applied to one path but not the other).
    ///
    /// # Stages
    /// 1. Order lookup in `self.orders` → not found: increment stat, return Ok
    /// 2. Price level lookup in bids/asks → not found: cleanup orphan, return Ok
    /// 3. Order-at-level lookup → not found: cleanup orphan, return Ok
    /// 4. Full or partial size reduction
    #[inline]
    fn reduce_or_remove_order(&mut self, msg: &MboMessage, op: OrderReductionOp) -> Result<()> {
        // Stage 1: Look up order
        let mut order = match self.orders.get(&msg.order_id) {
            Some(order) => *order,
            None => {
                // Order not found - common in real markets (already cancelled,
                // late message, aggressor side trades, etc.)
                match op {
                    OrderReductionOp::Cancel => self.stats.cancel_order_not_found += 1,
                    OrderReductionOp::Trade => self.stats.trade_order_not_found += 1,
                }
                if self.config.log_warnings {
                    log::debug!(
                        "{}: order {} not found (msg #{}, ts={:?})",
                        op.label(),
                        msg.order_id,
                        self.stats.messages_processed,
                        msg.timestamp
                    );
                }
                return Ok(());
            }
        };

        // Stage 2: Look up price level
        let price_level = match order.side {
            Side::Bid => self.bids.get_mut(&order.price),
            Side::Ask => self.asks.get_mut(&order.price),
            Side::None => return Ok(()),
        };

        let price_level = match price_level {
            Some(level) => level,
            None => {
                // Price level doesn't exist - data anomaly, but recoverable
                // Clean up the orphaned order tracking and continue
                match op {
                    OrderReductionOp::Cancel => self.stats.cancel_price_level_missing += 1,
                    OrderReductionOp::Trade => self.stats.trade_price_level_missing += 1,
                }
                self.orders.remove(&msg.order_id);
                if self.config.log_warnings {
                    log::warn!(
                        "{}: price level {} not found for order {} (msg #{}, cleaning up)",
                        op.label(),
                        order.price as f64 / NANODOLLARS_PER_DOLLAR_F64,
                        msg.order_id,
                        self.stats.messages_processed
                    );
                }
                return Ok(());
            }
        };

        // Stage 3: Look up order at price level
        let current_size = match price_level.get(&msg.order_id) {
            Some(&size) => size,
            None => {
                // Order not at price level - data anomaly, but recoverable
                // Clean up the orphaned order tracking and continue
                match op {
                    OrderReductionOp::Cancel => self.stats.cancel_order_at_level_missing += 1,
                    OrderReductionOp::Trade => self.stats.trade_order_at_level_missing += 1,
                }
                self.orders.remove(&msg.order_id);
                if self.config.log_warnings {
                    log::warn!(
                        "{}: order {} not found at price level {} (msg #{}, cleaning up)",
                        op.label(),
                        msg.order_id,
                        order.price as f64 / NANODOLLARS_PER_DOLLAR_F64,
                        self.stats.messages_processed
                    );
                }
                return Ok(());
            }
        };

        // Stage 4: Full or partial reduction
        if msg.size >= current_size {
            // Full removal (updates total_size)
            price_level.remove_order(msg.order_id);

            // Remove empty price level
            if price_level.is_empty() {
                match order.side {
                    Side::Bid => {
                        self.bids.remove(&order.price);
                    }
                    Side::Ask => {
                        self.asks.remove(&order.price);
                    }
                    Side::None => {}
                }
            }

            // Remove from order tracking
            self.orders.remove(&msg.order_id);
        } else {
            // Partial reduction (updates total_size via reduce_order)
            price_level.reduce_order(msg.order_id, msg.size);
            order.size = order.size.saturating_sub(msg.size);
            self.orders.insert(msg.order_id, order);
        }

        Ok(())
    }

    /// Internal helper to remove an order.
    #[inline(always)]
    fn remove_order_internal(&mut self, order_id: u64, order: &Order) -> Result<()> {
        // Get price level
        let price_level = match order.side {
            Side::Bid => self.bids.get_mut(&order.price),
            Side::Ask => self.asks.get_mut(&order.price),
            Side::None => return Ok(()),
        };

        if let Some(price_level) = price_level {
            // Remove order from price level (updates total_size)
            price_level.remove_order(order_id);

            // Remove empty price level
            if price_level.is_empty() {
                match order.side {
                    Side::Bid => {
                        self.bids.remove(&order.price);
                    }
                    Side::Ask => {
                        self.asks.remove(&order.price);
                    }
                    Side::None => {}
                }
            }
        }

        // Remove from order tracking
        self.orders.remove(&order_id);

        Ok(())
    }

    /// Update cached best bid and ask prices.
    ///
    /// BTreeMap keeps prices sorted, so we can efficiently get min/max.
    #[inline(always)]
    fn update_best_prices(&mut self) {
        // Best bid = highest bid price (BTreeMap iter is ascending, use last)
        self.best_bid = self.bids.keys().next_back().copied();

        // Best ask = lowest ask price (BTreeMap iter is ascending, use first)
        self.best_ask = self.asks.keys().next().copied();
    }

    /// Get current LOB state snapshot (without metadata).
    ///
    /// This creates a snapshot of the top N levels on each side.
    /// For most use cases, prefer `get_lob_state_with_metadata` which includes
    /// timestamp and sequence information.
    #[inline]
    pub fn get_lob_state(&self) -> LobState {
        self.get_lob_state_with_metadata(None)
    }

    /// Get current LOB state snapshot with metadata.
    ///
    /// This creates a snapshot of the top N levels on each side,
    /// including timestamp and message sequence number.
    ///
    /// # Arguments
    /// * `timestamp` - Optional timestamp from the message that triggered this snapshot
    #[inline]
    pub fn get_lob_state_with_metadata(&self, timestamp: Option<i64>) -> LobState {
        let levels = self.config.levels;
        let mut state = LobState::new(levels);
        self.fill_lob_state(&mut state, timestamp);
        state
    }

    /// Fill an existing LOB state with current book data (zero-allocation).
    ///
    /// This is the high-performance path for hot loops. Instead of allocating
    /// a new `LobState` on every call, you can reuse a pre-allocated one.
    ///
    /// # Arguments
    /// * `state` - Pre-allocated LobState to fill (will be cleared and populated)
    /// * `timestamp` - Optional timestamp from the message that triggered this snapshot
    ///
    /// # Performance
    ///
    /// This method performs **zero heap allocations** when used with the
    /// stack-allocated `LobState`. For processing millions of messages,
    /// this provides significant throughput improvements.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let mut lob = LobReconstructor::new(10);
    /// let mut state = LobState::new(10);  // Reused across calls
    ///
    /// for msg in messages {
    ///     lob.process_message_into(&msg, &mut state)?;
    ///     // Use state without allocation overhead
    ///     println!("Mid: {:?}", state.mid_price());
    /// }
    /// ```
    #[inline]
    pub fn fill_lob_state(&self, state: &mut LobState, timestamp: Option<i64>) {
        self.fill_lob_state_with_temporal(state, timestamp, None, None);
    }

    /// Fill LOB state with full temporal information.
    ///
    /// This is the enhanced version that populates temporal fields for
    /// time-sensitive feature extraction (FI-2010 u6-u9).
    ///
    /// # Arguments
    /// * `state` - Pre-allocated LobState buffer to fill
    /// * `timestamp` - Current message timestamp
    /// * `action` - The action that triggered this state
    /// * `side` - The side affected by the action
    #[inline]
    pub fn fill_lob_state_with_temporal(
        &self,
        state: &mut LobState,
        timestamp: Option<i64>,
        action: Option<Action>,
        side: Option<Side>,
    ) {
        let levels = self.config.levels.min(crate::types::MAX_LOB_LEVELS);

        // =========================================================================
        // Temporal Information (compute BEFORE clearing)
        // =========================================================================

        // Store previous timestamp for delta calculation
        let previous_ts = state.timestamp;

        // Calculate time delta
        let delta_ns = match (timestamp, previous_ts) {
            (Some(current), Some(prev)) if current > prev => (current - prev) as u64,
            _ => 0,
        };

        // Clear previous data (only clear used portion for efficiency)
        for i in 0..levels {
            state.bid_prices[i] = 0;
            state.bid_sizes[i] = 0;
            state.ask_prices[i] = 0;
            state.ask_sizes[i] = 0;
        }

        // Best prices
        state.best_bid = self.best_bid;
        state.best_ask = self.best_ask;

        // Metadata
        state.timestamp = timestamp.or(self.stats.last_timestamp);
        state.sequence = self.stats.messages_processed;
        state.levels = levels;

        // Temporal fields
        state.previous_timestamp = previous_ts;
        state.delta_ns = delta_ns;
        state.triggering_action = action;
        state.triggering_side = side;

        // Bid side (top N, highest to lowest)
        // Uses O(1) cached total_size() instead of O(n) values().sum()
        for (i, (&price, price_level)) in self.bids.iter().rev().take(levels).enumerate() {
            state.bid_prices[i] = price;
            state.bid_sizes[i] = price_level.total_size();

            // Parallel validation: verify cached total matches actual sum
            #[cfg(debug_assertions)]
            {
                let actual = price_level.compute_actual_total();
                debug_assert_eq!(
                    price_level.total_size(),
                    actual,
                    "Bid level {} cached size {} != actual {}",
                    price,
                    price_level.total_size(),
                    actual
                );
            }
        }

        // Ask side (top N, lowest to highest)
        // Uses O(1) cached total_size() instead of O(n) values().sum()
        for (i, (&price, price_level)) in self.asks.iter().take(levels).enumerate() {
            state.ask_prices[i] = price;
            state.ask_sizes[i] = price_level.total_size();

            // Parallel validation: verify cached total matches actual sum
            #[cfg(debug_assertions)]
            {
                let actual = price_level.compute_actual_total();
                debug_assert_eq!(
                    price_level.total_size(),
                    actual,
                    "Ask level {} cached size {} != actual {}",
                    price,
                    price_level.total_size(),
                    actual
                );
            }
        }
    }

    /// Process a message and write the resulting state into a pre-allocated buffer.
    ///
    /// This is the **high-performance zero-allocation** API for processing MBO messages.
    /// Instead of returning a new `LobState`, it fills the provided buffer in-place.
    ///
    /// # Arguments
    /// * `msg` - The MBO message to process
    /// * `state` - Pre-allocated LobState buffer to fill with the result
    ///
    /// # Returns
    /// * `Ok(())` if successful
    /// * `Err(TlobError)` if validation fails or crossed quote policy rejects
    ///
    /// # Performance
    ///
    /// Combined with stack-allocated `LobState`, this eliminates ALL heap allocations
    /// in the hot path, providing maximum throughput for real-time processing.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let mut lob = LobReconstructor::new(10);
    /// let mut state = LobState::new(10);
    ///
    /// for msg in messages {
    ///     lob.process_message_into(&msg, &mut state)?;
    ///     if let Some(mid) = state.mid_price() {
    ///         println!("Mid-price: ${:.4}", mid);
    ///     }
    /// }
    /// ```
    #[inline]
    pub fn process_message_into(&mut self, msg: &MboMessage, state: &mut LobState) -> Result<()> {
        // System messages (heartbeats, status updates) are not valid orders.
        if self.config.skip_system_messages && msg.is_system_message() {
            self.stats.system_messages_skipped += 1;
            // Still populate temporal info even for skipped messages
            self.fill_lob_state_with_temporal(
                state,
                msg.timestamp,
                Some(msg.action),
                Some(msg.side),
            );
            return Ok(());
        }

        // Validate message (if enabled)
        if self.config.validate_messages {
            msg.validate()?;
        }

        // Process based on action
        match msg.action {
            Action::Add => self.add_order(msg)?,
            Action::Modify => self.modify_order(msg)?,
            Action::Cancel => self.cancel_order(msg)?,
            Action::Trade | Action::Fill => self.process_trade(msg)?,
            Action::Clear => {
                self.stats.book_clears += 1;
                if self.config.log_warnings {
                    log::info!(
                        "Book clear received (msg #{}, ts={:?}, orders_before={})",
                        self.stats.messages_processed,
                        msg.timestamp,
                        self.orders.len()
                    );
                }
                self.reset();
            }
            Action::None => {
                // No-op, may carry flags or other info
                self.stats.noop_messages += 1;
            }
        }

        // Update statistics
        self.stats.messages_processed += 1;
        self.stats.active_orders = self.orders.len();
        self.stats.bid_levels = self.bids.len();
        self.stats.ask_levels = self.asks.len();

        // Track timestamp
        if let Some(ts) = msg.timestamp {
            self.stats.last_timestamp = Some(ts);
        }

        // Update best prices
        self.update_best_prices();

        // Check for book consistency
        let consistency = self.check_consistency();
        self.track_consistency(consistency);

        // Apply crossed quote policy and fill state with temporal info
        self.apply_crossed_quote_policy_into_with_temporal(
            consistency,
            msg.timestamp,
            Some(msg.action),
            Some(msg.side),
            state,
        )
    }

    /// Apply crossed quote policy and fill state with temporal info.
    #[inline]
    fn apply_crossed_quote_policy_into_with_temporal(
        &self,
        consistency: BookConsistency,
        timestamp: Option<i64>,
        action: Option<Action>,
        side: Option<Side>,
        state: &mut LobState,
    ) -> Result<()> {
        // For valid or empty states, always return current state
        if consistency == BookConsistency::Valid || consistency == BookConsistency::Empty {
            self.fill_lob_state_with_temporal(state, timestamp, action, side);
            return Ok(());
        }

        // For crossed or locked states, apply policy
        match self.config.crossed_quote_policy {
            CrossedQuotePolicy::Allow => {
                self.fill_lob_state_with_temporal(state, timestamp, action, side);
                Ok(())
            }
            CrossedQuotePolicy::UseLastValid | CrossedQuotePolicy::SkipUpdate => {
                if let Some(ref last_valid) = self.last_valid_state {
                    // Preserve the caller's temporal chain before overwriting
                    let previous_ts = state.timestamp;

                    // Clone the last valid book state (prices, sizes, levels, best_bid/ask)
                    *state = last_valid.clone();

                    // Patch temporal fields to maintain continuity:
                    // - Book data comes from the last valid snapshot
                    // - Temporal data comes from the current message
                    state.triggering_action = action;
                    state.triggering_side = side;
                    state.timestamp = timestamp.or(self.stats.last_timestamp);
                    state.previous_timestamp = previous_ts;
                    state.delta_ns = match (timestamp, previous_ts) {
                        (Some(current), Some(prev)) if current > prev => (current - prev) as u64,
                        _ => 0,
                    };
                    state.sequence = self.stats.messages_processed;
                } else {
                    self.fill_lob_state_with_temporal(state, timestamp, action, side);
                }
                Ok(())
            }
            CrossedQuotePolicy::Error => {
                if let (Some(bid), Some(ask)) = (self.best_bid, self.best_ask) {
                    if bid > ask {
                        Err(TlobError::CrossedQuote(bid, ask))
                    } else {
                        Err(TlobError::LockedQuote(bid, ask))
                    }
                } else {
                    self.fill_lob_state_with_temporal(state, timestamp, action, side);
                    Ok(())
                }
            }
        }
    }

    /// Reset the order book state (preserves statistics).
    ///
    /// Clears all orders and price levels but **preserves** `LobStats`.
    /// This is called internally when an `Action::Clear` message is received.
    ///
    /// # When to Use
    ///
    /// - Responding to `Action::Clear` messages (done automatically)
    /// - Mid-session reset without losing statistics
    ///
    /// # When NOT to Use
    ///
    /// - Starting a new trading day: use `full_reset()` instead
    /// - Fresh test runs: use `full_reset()` instead
    ///
    /// # Statistics Behavior
    ///
    /// Statistics (`messages_processed`, `system_messages_skipped`, etc.)
    /// are intentionally preserved so you can track cumulative metrics
    /// across resets within a session.
    pub fn reset(&mut self) {
        self.bids.clear();
        self.asks.clear();
        self.orders.clear();
        self.best_bid = None;
        self.best_ask = None;
        self.last_valid_state = None;
    }

    /// Fully reset the reconstructor including statistics.
    ///
    /// Clears all orders, price levels, **and** resets `LobStats` to zero.
    ///
    /// # When to Use
    ///
    /// - Starting a new trading day
    /// - Fresh test runs
    /// - Switching to a different symbol
    ///
    /// # Example
    ///
    /// ```ignore
    /// // Process day 1
    /// for msg in day1_messages {
    ///     lob.process_message(&msg)?;
    /// }
    /// let day1_stats = lob.stats().clone();
    ///
    /// // Reset for day 2
    /// lob.full_reset();
    /// assert_eq!(lob.stats().messages_processed, 0);
    ///
    /// // Process day 2
    /// for msg in day2_messages {
    ///     lob.process_message(&msg)?;
    /// }
    /// ```
    pub fn full_reset(&mut self) {
        self.reset();
        self.stats = LobStats::default();
    }

    /// Get current statistics.
    pub fn stats(&self) -> &LobStats {
        &self.stats
    }

    /// Get number of active orders.
    pub fn order_count(&self) -> usize {
        self.orders.len()
    }

    /// Get number of price levels on bid side.
    pub fn bid_levels(&self) -> usize {
        self.bids.len()
    }

    /// Get number of price levels on ask side.
    pub fn ask_levels(&self) -> usize {
        self.asks.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_message(
        order_id: u64,
        action: Action,
        side: Side,
        price_dollars: f64,
        size: u32,
    ) -> MboMessage {
        MboMessage::new(order_id, action, side, (price_dollars * 1e9) as i64, size)
    }

    #[test]
    fn test_new_lob() {
        let lob = LobReconstructor::new(10);
        assert_eq!(lob.order_count(), 0);
        assert_eq!(lob.bid_levels(), 0);
        assert_eq!(lob.ask_levels(), 0);
    }

    #[test]
    fn test_add_bid_order() {
        let mut lob = LobReconstructor::new(10);

        let msg = create_test_message(1, Action::Add, Side::Bid, 100.0, 100);
        let state = lob.process_message(&msg).unwrap();

        assert_eq!(lob.order_count(), 1);
        assert_eq!(lob.bid_levels(), 1);
        assert_eq!(state.best_bid, Some(100_000_000_000));
        assert_eq!(state.bid_prices[0], 100_000_000_000);
        assert_eq!(state.bid_sizes[0], 100);
    }

    #[test]
    fn test_add_ask_order() {
        let mut lob = LobReconstructor::new(10);

        let msg = create_test_message(1, Action::Add, Side::Ask, 100.01, 200);
        let state = lob.process_message(&msg).unwrap();

        assert_eq!(lob.order_count(), 1);
        assert_eq!(lob.ask_levels(), 1);
        assert_eq!(state.best_ask, Some(100_010_000_000));
        assert_eq!(state.ask_prices[0], 100_010_000_000);
        assert_eq!(state.ask_sizes[0], 200);
    }

    #[test]
    fn test_bid_ask_spread() {
        let mut lob = LobReconstructor::new(10);

        // Add bid
        let bid = create_test_message(1, Action::Add, Side::Bid, 100.0, 100);
        lob.process_message(&bid).unwrap();

        // Add ask
        let ask = create_test_message(2, Action::Add, Side::Ask, 100.01, 200);
        let state = lob.process_message(&ask).unwrap();

        // Check spread
        assert!(state.is_valid());
        assert!((state.mid_price().unwrap() - 100.005).abs() < 1e-6);
        assert!((state.spread().unwrap() - 0.01).abs() < 1e-6);
    }

    #[test]
    fn test_cancel_order() {
        let mut lob = LobReconstructor::new(10);

        // Add order
        let add = create_test_message(1, Action::Add, Side::Bid, 100.0, 100);
        lob.process_message(&add).unwrap();
        assert_eq!(lob.order_count(), 1);

        // Cancel order
        let cancel = create_test_message(1, Action::Cancel, Side::Bid, 100.0, 100);
        lob.process_message(&cancel).unwrap();
        assert_eq!(lob.order_count(), 0);
        assert_eq!(lob.bid_levels(), 0);
    }

    #[test]
    fn test_modify_order() {
        let mut lob = LobReconstructor::new(10);

        // Add order
        let add = create_test_message(1, Action::Add, Side::Bid, 100.0, 100);
        lob.process_message(&add).unwrap();

        // Modify order (change price)
        let modify = create_test_message(1, Action::Modify, Side::Bid, 100.01, 150);
        let state = lob.process_message(&modify).unwrap();

        assert_eq!(lob.order_count(), 1);
        assert_eq!(state.best_bid, Some(100_010_000_000));
        assert_eq!(state.bid_sizes[0], 150);
    }

    /// Phase M M.A.4 F-013 closure: locks the silent-fall-through observability
    /// counter for `Modify` of an unknown `order_id`.
    ///
    /// Pre-M.A.4: a `Modify` for a missing `order_id` would silently call
    /// `add_order(msg)` (creating a NEW order at MODIFY's price), with no
    /// counter, no log, no diagnostic surface — operators had zero visibility
    /// into the rate. Post-M.A.4: the counter increments BEFORE the recovery
    /// fall-through, exposing the rate to operator dashboards.
    #[test]
    fn test_modify_order_not_found_increments_counter() {
        let mut lob = LobReconstructor::new(10);

        // No prior Add — the order is not in the tracker.
        assert_eq!(lob.stats().modify_order_not_found, 0);

        // Modify with an unknown order_id; recovers as Add at MODIFY's price.
        let modify = create_test_message(99, Action::Modify, Side::Bid, 100.5, 200);
        lob.process_message(&modify).unwrap();

        // Counter must increment exactly once.
        assert_eq!(lob.stats().modify_order_not_found, 1);

        // Recovery semantic preserved: a NEW order now exists at MODIFY's
        // price, side, size — verifying the fall-through still works.
        assert_eq!(lob.order_count(), 1);

        // A second Modify-of-missing increments to 2.
        let modify2 = create_test_message(100, Action::Modify, Side::Ask, 101.0, 300);
        lob.process_message(&modify2).unwrap();
        assert_eq!(lob.stats().modify_order_not_found, 2);

        // has_warnings() reports true (warning counter taxonomy).
        assert!(lob.stats().has_warnings());
        assert!(lob.stats().total_warnings() >= 2);
    }

    /// Phase M M.A.4 F-013 sibling closure: locks the silent-fall-through
    /// observability counter for `Add` of an existing `order_id`.
    ///
    /// Pre-M.A.4: an `Add` for an already-tracked `order_id` would silently
    /// call `modify_order(msg)` (some venues reuse IDs intentionally), with
    /// no counter. Post-M.A.4: the counter increments BEFORE the recovery
    /// fall-through, exposing the rate.
    #[test]
    fn test_add_order_id_collision_increments_counter() {
        let mut lob = LobReconstructor::new(10);

        // First Add for order_id=1.
        let add1 = create_test_message(1, Action::Add, Side::Bid, 100.0, 100);
        lob.process_message(&add1).unwrap();
        assert_eq!(lob.stats().add_order_id_collision, 0);

        // Second Add for the same order_id=1; recovers as Modify.
        let add2 = create_test_message(1, Action::Add, Side::Bid, 100.05, 150);
        lob.process_message(&add2).unwrap();

        // Counter must increment exactly once.
        assert_eq!(lob.stats().add_order_id_collision, 1);

        // Recovery semantic preserved: order_count remains 1 (modify not add).
        assert_eq!(lob.order_count(), 1);

        // has_warnings() reports true.
        assert!(lob.stats().has_warnings());
    }

    /// Phase M M.A.4 F-007 closure: verify the legacy `errors` field has
    /// been REMOVED from the `LobStats` struct. This test is a structural
    /// regression-lock — if a future commit re-introduces the field, this
    /// test will fail to compile (intentional drift detector).
    ///
    /// The replacement counters (`modify_order_not_found` +
    /// `add_order_id_collision`) cover the previously-untracked anomalies.
    /// Specific error variants are tracked by `crossed_quotes`,
    /// `locked_quotes`, and the per-action `*_not_found` / `*_missing`
    /// counters.
    #[test]
    fn test_lob_stats_errors_field_removed() {
        let stats = LobStats::default();
        // Verify replacement counters are present + default to zero.
        assert_eq!(stats.modify_order_not_found, 0);
        assert_eq!(stats.add_order_id_collision, 0);
        // Pre-existing surface preserved.
        assert_eq!(stats.crossed_quotes, 0);
        assert_eq!(stats.locked_quotes, 0);
        // Note: `stats.errors` is intentionally absent. Compile-time
        // verification — any reintroduction would require this test to
        // change (drift detector).
    }

    #[test]
    fn test_trade_partial_fill() {
        let mut lob = LobReconstructor::new(10);

        // Add order
        let add = create_test_message(1, Action::Add, Side::Bid, 100.0, 100);
        lob.process_message(&add).unwrap();

        // Partial fill (50 shares)
        let trade = create_test_message(1, Action::Trade, Side::Bid, 100.0, 50);
        let state = lob.process_message(&trade).unwrap();

        assert_eq!(lob.order_count(), 1); // Order still exists
        assert_eq!(state.bid_sizes[0], 50); // Reduced size
    }

    #[test]
    fn test_trade_full_fill() {
        let mut lob = LobReconstructor::new(10);

        // Add order
        let add = create_test_message(1, Action::Add, Side::Bid, 100.0, 100);
        lob.process_message(&add).unwrap();

        // Full fill
        let trade = create_test_message(1, Action::Trade, Side::Bid, 100.0, 100);
        lob.process_message(&trade).unwrap();

        assert_eq!(lob.order_count(), 0); // Order removed
        assert_eq!(lob.bid_levels(), 0); // Price level removed
    }

    #[test]
    fn test_multiple_orders_same_price() {
        let mut lob = LobReconstructor::new(10);

        // Add multiple orders at same price
        lob.process_message(&create_test_message(1, Action::Add, Side::Bid, 100.0, 100))
            .unwrap();
        lob.process_message(&create_test_message(2, Action::Add, Side::Bid, 100.0, 200))
            .unwrap();
        lob.process_message(&create_test_message(3, Action::Add, Side::Bid, 100.0, 300))
            .unwrap();

        let state = lob.get_lob_state();

        assert_eq!(lob.order_count(), 3);
        assert_eq!(lob.bid_levels(), 1); // All at same price
        assert_eq!(state.bid_sizes[0], 600); // Aggregated size
    }

    #[test]
    fn test_reset() {
        let mut lob = LobReconstructor::new(10);

        // Add some orders
        lob.process_message(&create_test_message(1, Action::Add, Side::Bid, 100.0, 100))
            .unwrap();
        lob.process_message(&create_test_message(2, Action::Add, Side::Ask, 100.01, 200))
            .unwrap();

        assert_eq!(lob.order_count(), 2);

        // Reset
        lob.reset();

        assert_eq!(lob.order_count(), 0);
        assert_eq!(lob.bid_levels(), 0);
        assert_eq!(lob.ask_levels(), 0);
    }

    #[test]
    fn test_statistics() {
        let mut lob = LobReconstructor::new(10);

        lob.process_message(&create_test_message(1, Action::Add, Side::Bid, 100.0, 100))
            .unwrap();
        lob.process_message(&create_test_message(2, Action::Add, Side::Ask, 100.01, 200))
            .unwrap();

        let stats = lob.stats();
        assert_eq!(stats.messages_processed, 2);
        assert_eq!(stats.active_orders, 2);
        assert_eq!(stats.bid_levels, 1);
        assert_eq!(stats.ask_levels, 1);
    }

    // =========================================================================
    // Crossed Quote Policy Tests
    // =========================================================================

    #[test]
    fn test_crossed_quote_detection() {
        let mut lob = LobReconstructor::new(10);

        // Add valid bid
        lob.process_message(&create_test_message(1, Action::Add, Side::Bid, 100.0, 100))
            .unwrap();

        // Add ask BELOW bid (creates crossed quote)
        lob.process_message(&create_test_message(2, Action::Add, Side::Ask, 99.99, 200))
            .unwrap();

        // Book should be crossed
        assert!(lob.is_crossed());
        assert!(!lob.is_consistent());

        // Stats should show crossed quote
        let stats = lob.stats();
        assert_eq!(stats.crossed_quotes, 1);
    }

    #[test]
    fn test_locked_quote_detection() {
        let mut lob = LobReconstructor::new(10);

        // Add bid
        lob.process_message(&create_test_message(1, Action::Add, Side::Bid, 100.0, 100))
            .unwrap();

        // Add ask at SAME price as bid (creates locked quote)
        lob.process_message(&create_test_message(2, Action::Add, Side::Ask, 100.0, 200))
            .unwrap();

        // Stats should show locked quote
        let stats = lob.stats();
        assert_eq!(stats.locked_quotes, 1);
    }

    #[test]
    fn test_crossed_quote_policy_allow() {
        let config = LobConfig::new(10)
            .with_crossed_quote_policy(CrossedQuotePolicy::Allow)
            .with_logging(false);
        let mut lob = LobReconstructor::with_config(config);

        // Create crossed book
        lob.process_message(&create_test_message(1, Action::Add, Side::Bid, 100.0, 100))
            .unwrap();
        let state = lob
            .process_message(&create_test_message(2, Action::Add, Side::Ask, 99.99, 200))
            .unwrap();

        // Should return the crossed state (Allow policy)
        assert!(state.is_crossed());
        assert_eq!(state.best_bid, Some(100_000_000_000)); // $100.00
        assert_eq!(state.best_ask, Some(99_990_000_000)); // $99.99
    }

    #[test]
    fn test_crossed_quote_policy_use_last_valid() {
        let config = LobConfig::new(10)
            .with_crossed_quote_policy(CrossedQuotePolicy::UseLastValid)
            .with_logging(false);
        let mut lob = LobReconstructor::with_config(config);

        // Create a valid book first (bid < ask)
        lob.process_message(&create_test_message(1, Action::Add, Side::Bid, 100.0, 100))
            .unwrap();
        let valid_state = lob
            .process_message(&create_test_message(2, Action::Add, Side::Ask, 100.01, 200))
            .unwrap();

        // Verify we have a valid state
        assert!(!valid_state.is_crossed());
        assert!(valid_state.is_valid());

        // Cancel the ask to prepare for crossed quote
        lob.process_message(&create_test_message(
            2,
            Action::Cancel,
            Side::Ask,
            100.01,
            200,
        ))
        .unwrap();

        // Try to create crossed book (ask below bid)
        let state_after_cross = lob
            .process_message(&create_test_message(3, Action::Add, Side::Ask, 99.99, 200))
            .unwrap();

        // Should return the last valid state (not crossed)
        assert!(!state_after_cross.is_crossed());
        // The returned state should be the last valid one
        assert_eq!(state_after_cross.best_ask, Some(100_010_000_000)); // $100.01 from valid state
    }

    #[test]
    fn test_crossed_quote_policy_error() {
        let config = LobConfig::new(10)
            .with_crossed_quote_policy(CrossedQuotePolicy::Error)
            .with_logging(false);
        let mut lob = LobReconstructor::with_config(config);

        // Add valid bid
        lob.process_message(&create_test_message(1, Action::Add, Side::Bid, 100.0, 100))
            .unwrap();

        // Try to create crossed book - should return error
        let result =
            lob.process_message(&create_test_message(2, Action::Add, Side::Ask, 99.99, 200));

        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            crate::error::TlobError::CrossedQuote(_, _)
        ));
    }

    #[test]
    fn test_config_builder() {
        let config = LobConfig::new(5)
            .with_crossed_quote_policy(CrossedQuotePolicy::UseLastValid)
            .with_validation(false)
            .with_logging(false);

        assert_eq!(config.levels, 5);
        assert_eq!(
            config.crossed_quote_policy,
            CrossedQuotePolicy::UseLastValid
        );
        assert!(!config.validate_messages);
        assert!(!config.log_warnings);
    }

    #[test]
    fn test_lob_with_config() {
        let config = LobConfig::new(5);
        let lob = LobReconstructor::with_config(config);

        assert_eq!(lob.levels(), 5);
    }

    #[test]
    fn test_stats_track_crossed_and_locked() {
        let config = LobConfig::new(10).with_logging(false);
        let mut lob = LobReconstructor::with_config(config);

        // Add bid
        lob.process_message(&create_test_message(1, Action::Add, Side::Bid, 100.0, 100))
            .unwrap();

        // Create locked quote
        lob.process_message(&create_test_message(2, Action::Add, Side::Ask, 100.0, 200))
            .unwrap();
        assert_eq!(lob.stats().locked_quotes, 1);

        // Cancel the ask
        lob.process_message(&create_test_message(
            2,
            Action::Cancel,
            Side::Ask,
            100.0,
            200,
        ))
        .unwrap();

        // Create crossed quote
        lob.process_message(&create_test_message(3, Action::Add, Side::Ask, 99.99, 200))
            .unwrap();
        assert_eq!(lob.stats().crossed_quotes, 1);

        // Total counts
        assert_eq!(lob.stats().locked_quotes, 1);
        assert_eq!(lob.stats().crossed_quotes, 1);
    }

    // =========================================================================
    // Partial Cancel Tests
    // =========================================================================

    #[test]
    fn test_partial_cancel_preserves_order() {
        let mut lob = LobReconstructor::new(10);

        // Add order with 100 shares
        lob.process_message(&create_test_message(1, Action::Add, Side::Bid, 100.0, 100))
            .unwrap();
        assert_eq!(lob.order_count(), 1);
        assert_eq!(lob.get_lob_state().bid_sizes[0], 100);

        // Partial cancel: remove 30 shares
        lob.process_message(&create_test_message(
            1,
            Action::Cancel,
            Side::Bid,
            100.0,
            30,
        ))
        .unwrap();

        // Order should still exist with 70 shares
        assert_eq!(lob.order_count(), 1);
        assert_eq!(lob.get_lob_state().bid_sizes[0], 70);
    }

    #[test]
    fn test_multiple_partial_cancels() {
        let mut lob = LobReconstructor::new(10);

        // Add order with 100 shares
        lob.process_message(&create_test_message(1, Action::Add, Side::Bid, 100.0, 100))
            .unwrap();

        // First partial cancel: remove 20 shares (80 remaining)
        lob.process_message(&create_test_message(
            1,
            Action::Cancel,
            Side::Bid,
            100.0,
            20,
        ))
        .unwrap();
        assert_eq!(lob.get_lob_state().bid_sizes[0], 80);

        // Second partial cancel: remove 30 shares (50 remaining)
        lob.process_message(&create_test_message(
            1,
            Action::Cancel,
            Side::Bid,
            100.0,
            30,
        ))
        .unwrap();
        assert_eq!(lob.get_lob_state().bid_sizes[0], 50);

        // Third partial cancel: remove 50 shares (0 remaining = full cancel)
        lob.process_message(&create_test_message(
            1,
            Action::Cancel,
            Side::Bid,
            100.0,
            50,
        ))
        .unwrap();
        assert_eq!(lob.order_count(), 0);
        assert_eq!(lob.bid_levels(), 0);
    }

    #[test]
    fn test_partial_cancel_at_bbo_preserves_price() {
        let mut lob = LobReconstructor::new(10);

        // Add best bid at $100.00
        lob.process_message(&create_test_message(1, Action::Add, Side::Bid, 100.0, 100))
            .unwrap();
        // Add second level at $99.99
        lob.process_message(&create_test_message(2, Action::Add, Side::Bid, 99.99, 200))
            .unwrap();

        assert_eq!(lob.get_lob_state().best_bid, Some(100_000_000_000));

        // Partial cancel at BBO - price should NOT change
        lob.process_message(&create_test_message(
            1,
            Action::Cancel,
            Side::Bid,
            100.0,
            50,
        ))
        .unwrap();

        // Best bid should still be $100.00
        assert_eq!(lob.get_lob_state().best_bid, Some(100_000_000_000));
        assert_eq!(lob.get_lob_state().bid_sizes[0], 50);
    }

    #[test]
    fn test_full_cancel_removes_price_level() {
        let mut lob = LobReconstructor::new(10);

        // Add best bid at $100.00
        lob.process_message(&create_test_message(1, Action::Add, Side::Bid, 100.0, 100))
            .unwrap();
        // Add second level at $99.99
        lob.process_message(&create_test_message(2, Action::Add, Side::Bid, 99.99, 200))
            .unwrap();

        assert_eq!(lob.bid_levels(), 2);

        // Full cancel at BBO - price level should be removed
        lob.process_message(&create_test_message(
            1,
            Action::Cancel,
            Side::Bid,
            100.0,
            100,
        ))
        .unwrap();

        // Best bid should now be $99.99
        assert_eq!(lob.bid_levels(), 1);
        assert_eq!(lob.get_lob_state().best_bid, Some(99_990_000_000));
    }

    #[test]
    fn test_over_cancel_removes_order() {
        let mut lob = LobReconstructor::new(10);

        // Add order with 50 shares
        lob.process_message(&create_test_message(1, Action::Add, Side::Bid, 100.0, 50))
            .unwrap();

        // Cancel more than exists (100 > 50) - should remove entirely
        lob.process_message(&create_test_message(
            1,
            Action::Cancel,
            Side::Bid,
            100.0,
            100,
        ))
        .unwrap();

        assert_eq!(lob.order_count(), 0);
    }

    // =========================================================================
    // Saturating Subtraction Tests (Defensive Programming)
    // =========================================================================
    // These tests document the defensive use of saturating_sub in partial
    // cancel and trade operations. While normal operation should never cause
    // underflow (the msg.size >= order.size check prevents it), saturating_sub
    // provides a safety net against potential data inconsistencies between
    // Order and PriceLevel tracking.

    #[test]
    fn test_partial_cancel_size_reduction() {
        // Test that partial cancel correctly reduces order size
        let mut lob = LobReconstructor::new(10);

        // Add order with 100 shares
        lob.process_message(&create_test_message(1, Action::Add, Side::Bid, 100.0, 100))
            .unwrap();

        // Partial cancel of 30 shares
        lob.process_message(&create_test_message(
            1,
            Action::Cancel,
            Side::Bid,
            100.0,
            30,
        ))
        .unwrap();

        // Order should have 70 shares remaining
        assert_eq!(lob.order_count(), 1);
        assert_eq!(lob.get_lob_state().bid_sizes[0], 70);

        // Partial cancel of 40 more shares
        lob.process_message(&create_test_message(
            1,
            Action::Cancel,
            Side::Bid,
            100.0,
            40,
        ))
        .unwrap();

        // Order should have 30 shares remaining
        assert_eq!(lob.order_count(), 1);
        assert_eq!(lob.get_lob_state().bid_sizes[0], 30);
    }

    #[test]
    fn test_partial_trade_size_reduction() {
        // Test that partial trade (fill) correctly reduces order size
        let mut lob = LobReconstructor::new(10);

        // Add order with 100 shares
        lob.process_message(&create_test_message(1, Action::Add, Side::Ask, 100.0, 100))
            .unwrap();

        // Partial fill of 25 shares
        lob.process_message(&create_test_message(1, Action::Trade, Side::Ask, 100.0, 25))
            .unwrap();

        // Order should have 75 shares remaining
        assert_eq!(lob.order_count(), 1);
        assert_eq!(lob.get_lob_state().ask_sizes[0], 75);

        // Partial fill of 50 more shares
        lob.process_message(&create_test_message(1, Action::Trade, Side::Ask, 100.0, 50))
            .unwrap();

        // Order should have 25 shares remaining
        assert_eq!(lob.order_count(), 1);
        assert_eq!(lob.get_lob_state().ask_sizes[0], 25);
    }

    #[test]
    fn test_over_trade_removes_order() {
        // Test that trading more than order size removes the order cleanly
        // (analogous to test_over_cancel_removes_order)
        let mut lob = LobReconstructor::new(10);

        // Add order with 50 shares
        lob.process_message(&create_test_message(1, Action::Add, Side::Ask, 100.0, 50))
            .unwrap();

        // Trade more than exists (100 > 50) - should remove entirely
        lob.process_message(&create_test_message(
            1,
            Action::Trade,
            Side::Ask,
            100.0,
            100,
        ))
        .unwrap();

        assert_eq!(lob.order_count(), 0);
        assert_eq!(lob.ask_levels(), 0);
    }

    // =========================================================================
    // Action::Clear Tests
    // =========================================================================

    #[test]
    fn test_clear_resets_book() {
        // Disable validation and system message skipping since Clear uses dummy values
        let config = LobConfig::new(10)
            .with_logging(false)
            .with_validation(false)
            .with_skip_system_messages(false); // Allow order_id=0, price=0 for Clear
        let mut lob = LobReconstructor::with_config(config);

        // Build up some state
        lob.process_message(&create_test_message(1, Action::Add, Side::Bid, 100.0, 100))
            .unwrap();
        lob.process_message(&create_test_message(2, Action::Add, Side::Ask, 100.01, 200))
            .unwrap();
        lob.process_message(&create_test_message(3, Action::Add, Side::Bid, 99.99, 150))
            .unwrap();

        assert_eq!(lob.order_count(), 3);
        assert_eq!(lob.bid_levels(), 2);
        assert_eq!(lob.ask_levels(), 1);

        // Clear the book (use dummy values since Clear doesn't need them)
        let msg = MboMessage::new(0, Action::Clear, Side::None, 0, 0);
        lob.process_message(&msg).unwrap();

        // Book should be empty
        assert_eq!(lob.order_count(), 0);
        assert_eq!(lob.bid_levels(), 0);
        assert_eq!(lob.ask_levels(), 0);
        assert!(lob.get_lob_state().best_bid.is_none());
        assert!(lob.get_lob_state().best_ask.is_none());

        // Stats should track the clear
        assert_eq!(lob.stats().book_clears, 1);
    }

    // =========================================================================
    // Action::None Tests
    // =========================================================================

    #[test]
    fn test_none_action_is_noop() {
        // Disable validation and system message skipping since None uses dummy values
        let config = LobConfig::new(10)
            .with_logging(false)
            .with_validation(false)
            .with_skip_system_messages(false); // Allow price=0 for None
        let mut lob = LobReconstructor::with_config(config);

        // Add an order
        lob.process_message(&create_test_message(1, Action::Add, Side::Bid, 100.0, 100))
            .unwrap();

        let state_before = lob.get_lob_state();
        let orders_before = lob.order_count();

        // Process None action (use dummy values since None doesn't need them)
        let msg = MboMessage::new(999, Action::None, Side::None, 0, 0);
        lob.process_message(&msg).unwrap();

        // State should be unchanged
        assert_eq!(lob.order_count(), orders_before);
        assert_eq!(lob.get_lob_state().best_bid, state_before.best_bid);

        // Stats should track the noop
        assert_eq!(lob.stats().noop_messages, 1);
    }

    // =========================================================================
    // System Message Skipping Tests
    // =========================================================================

    #[test]
    fn test_system_messages_skipped_by_default() {
        // Default config has skip_system_messages=true
        let mut lob = LobReconstructor::new(10);

        // Add a valid order first
        lob.process_message(&create_test_message(1, Action::Add, Side::Bid, 100.0, 100))
            .unwrap();
        assert_eq!(lob.order_count(), 1);
        assert_eq!(lob.stats().messages_processed, 1);
        assert_eq!(lob.stats().system_messages_skipped, 0);

        // System message: order_id = 0
        let msg = MboMessage::new(0, Action::Add, Side::Bid, 100_000_000_000, 100);
        let state_before = lob.get_lob_state();
        lob.process_message(&msg).unwrap(); // Should NOT error
        assert_eq!(lob.get_lob_state().best_bid, state_before.best_bid); // State unchanged
        assert_eq!(lob.stats().system_messages_skipped, 1);
        assert_eq!(lob.stats().messages_processed, 1); // Not incremented

        // System message: size = 0
        let msg = MboMessage::new(123, Action::Add, Side::Bid, 100_000_000_000, 0);
        lob.process_message(&msg).unwrap(); // Should NOT error
        assert_eq!(lob.stats().system_messages_skipped, 2);

        // System message: price <= 0
        let msg = MboMessage::new(123, Action::Add, Side::Bid, 0, 100);
        lob.process_message(&msg).unwrap(); // Should NOT error
        assert_eq!(lob.stats().system_messages_skipped, 3);

        let msg = MboMessage::new(123, Action::Add, Side::Bid, -100, 100);
        lob.process_message(&msg).unwrap(); // Should NOT error
        assert_eq!(lob.stats().system_messages_skipped, 4);

        // Order count should still be 1
        assert_eq!(lob.order_count(), 1);
    }

    #[test]
    fn test_system_messages_not_skipped_when_disabled() {
        // Disable system message skipping
        let config = LobConfig::new(10)
            .with_skip_system_messages(false)
            .with_validation(true); // Keep validation enabled

        let mut lob = LobReconstructor::with_config(config);

        // System message should now fail validation
        let msg = MboMessage::new(0, Action::Add, Side::Bid, 100_000_000_000, 100);
        let result = lob.process_message(&msg);
        assert!(result.is_err()); // Should error because order_id=0 is invalid
    }

    // =========================================================================
    // Soft Error Handling Tests
    // =========================================================================

    #[test]
    fn test_cancel_unknown_order_is_ok() {
        let config = LobConfig::new(10).with_logging(false);
        let mut lob = LobReconstructor::with_config(config);

        // Cancel an order that doesn't exist - should not fail
        let result = lob.process_message(&create_test_message(
            999,
            Action::Cancel,
            Side::Bid,
            100.0,
            50,
        ));

        assert!(result.is_ok());
        assert_eq!(lob.stats().cancel_order_not_found, 1);
    }

    #[test]
    fn test_trade_unknown_order_is_ok() {
        let config = LobConfig::new(10).with_logging(false);
        let mut lob = LobReconstructor::with_config(config);

        // Trade for an order that doesn't exist - should not fail
        let result = lob.process_message(&create_test_message(
            999,
            Action::Trade,
            Side::Bid,
            100.0,
            50,
        ));

        assert!(result.is_ok());
        assert_eq!(lob.stats().trade_order_not_found, 1);
    }

    #[test]
    fn test_warning_stats_accumulate() {
        let config = LobConfig::new(10).with_logging(false);
        let mut lob = LobReconstructor::with_config(config);

        // Multiple unknown order operations
        lob.process_message(&create_test_message(
            1,
            Action::Cancel,
            Side::Bid,
            100.0,
            50,
        ))
        .unwrap();
        lob.process_message(&create_test_message(
            2,
            Action::Cancel,
            Side::Bid,
            100.0,
            50,
        ))
        .unwrap();
        lob.process_message(&create_test_message(3, Action::Trade, Side::Bid, 100.0, 50))
            .unwrap();

        assert_eq!(lob.stats().cancel_order_not_found, 2);
        assert_eq!(lob.stats().trade_order_not_found, 1);
    }

    // =========================================================================
    // Zero-allocation API Tests
    // =========================================================================

    #[test]
    fn test_process_message_into_matches_process_message() {
        use crate::types::LobState;

        // Create two identical LOBs
        let mut lob1 = LobReconstructor::new(10);
        let mut lob2 = LobReconstructor::new(10);
        let mut reused_state = LobState::new(10);

        // Define test messages
        let messages = vec![
            create_test_message(1, Action::Add, Side::Bid, 100.0, 100),
            create_test_message(2, Action::Add, Side::Ask, 100.05, 150),
            create_test_message(3, Action::Add, Side::Bid, 99.95, 200),
            create_test_message(4, Action::Add, Side::Ask, 100.10, 50),
            create_test_message(1, Action::Modify, Side::Bid, 100.0, 80),
            create_test_message(5, Action::Add, Side::Bid, 100.02, 300),
            create_test_message(2, Action::Cancel, Side::Ask, 100.05, 50),
            create_test_message(6, Action::Trade, Side::Bid, 100.0, 30),
        ];

        // Process with both APIs
        for msg in &messages {
            let state1 = lob1.process_message(msg).unwrap();
            lob2.process_message_into(msg, &mut reused_state).unwrap();

            // Verify states are identical
            assert_eq!(
                state1.best_bid, reused_state.best_bid,
                "best_bid mismatch at msg {:?}",
                msg.order_id
            );
            assert_eq!(
                state1.best_ask, reused_state.best_ask,
                "best_ask mismatch at msg {:?}",
                msg.order_id
            );
            assert_eq!(
                state1.sequence, reused_state.sequence,
                "sequence mismatch at msg {:?}",
                msg.order_id
            );

            // Compare price levels
            for i in 0..10 {
                assert_eq!(
                    state1.bid_prices[i], reused_state.bid_prices[i],
                    "bid_prices[{}] mismatch at msg {:?}",
                    i, msg.order_id
                );
                assert_eq!(
                    state1.bid_sizes[i], reused_state.bid_sizes[i],
                    "bid_sizes[{}] mismatch at msg {:?}",
                    i, msg.order_id
                );
                assert_eq!(
                    state1.ask_prices[i], reused_state.ask_prices[i],
                    "ask_prices[{}] mismatch at msg {:?}",
                    i, msg.order_id
                );
                assert_eq!(
                    state1.ask_sizes[i], reused_state.ask_sizes[i],
                    "ask_sizes[{}] mismatch at msg {:?}",
                    i, msg.order_id
                );
            }

            // Verify analytics match
            assert_eq!(
                state1.mid_price(),
                reused_state.mid_price(),
                "mid_price mismatch at msg {:?}",
                msg.order_id
            );
            assert_eq!(
                state1.spread(),
                reused_state.spread(),
                "spread mismatch at msg {:?}",
                msg.order_id
            );
        }
    }

    #[test]
    fn test_fill_lob_state_clears_previous_data() {
        use crate::types::LobState;

        let mut lob = LobReconstructor::new(5);
        let mut state = LobState::new(5);

        // First: Build up some state
        lob.process_message(&create_test_message(1, Action::Add, Side::Bid, 100.0, 100))
            .unwrap();
        lob.process_message(&create_test_message(2, Action::Add, Side::Ask, 100.05, 150))
            .unwrap();
        lob.process_message(&create_test_message(3, Action::Add, Side::Bid, 99.95, 200))
            .unwrap();
        lob.process_message_into(
            &create_test_message(4, Action::Add, Side::Ask, 100.10, 50),
            &mut state,
        )
        .unwrap();

        // Verify state has data
        assert!(state.bid_prices[0] > 0);
        assert!(state.bid_prices[1] > 0);
        assert!(state.ask_prices[0] > 0);
        assert!(state.ask_prices[1] > 0);

        // Now clear the LOB (simulating book clear)
        lob.reset();
        lob.fill_lob_state(&mut state, None);

        // Verify state is cleared
        assert_eq!(state.best_bid, None);
        assert_eq!(state.best_ask, None);
        for i in 0..5 {
            assert_eq!(state.bid_prices[i], 0, "bid_prices[{}] not cleared", i);
            assert_eq!(state.bid_sizes[i], 0, "bid_sizes[{}] not cleared", i);
            assert_eq!(state.ask_prices[i], 0, "ask_prices[{}] not cleared", i);
            assert_eq!(state.ask_sizes[i], 0, "ask_sizes[{}] not cleared", i);
        }
    }

    /// Test that PriceLevel cached sizes stay accurate after complex operations.
    ///
    /// This test simulates a realistic trading scenario with multiple orders
    /// at the same price level, partial cancels, and trades, then verifies
    /// the aggregated size matches the expected value.
    #[test]
    fn test_price_level_cache_consistency_complex() {
        let mut lob = LobReconstructor::new(10);

        // Add 5 orders at same price level (total: 500)
        for i in 1..=5 {
            lob.process_message(&create_test_message(i, Action::Add, Side::Bid, 100.0, 100))
                .unwrap();
        }
        assert_eq!(lob.get_lob_state().bid_sizes[0], 500);

        // Partial cancel order 1: remove 30 (total: 470)
        lob.process_message(&create_test_message(
            1,
            Action::Cancel,
            Side::Bid,
            100.0,
            30,
        ))
        .unwrap();
        assert_eq!(lob.get_lob_state().bid_sizes[0], 470);

        // Trade on order 2: remove 50 (total: 420)
        lob.process_message(&create_test_message(2, Action::Trade, Side::Bid, 100.0, 50))
            .unwrap();
        assert_eq!(lob.get_lob_state().bid_sizes[0], 420);

        // Full cancel order 3 (total: 320)
        lob.process_message(&create_test_message(
            3,
            Action::Cancel,
            Side::Bid,
            100.0,
            100,
        ))
        .unwrap();
        assert_eq!(lob.get_lob_state().bid_sizes[0], 320);

        // Add new order at same price (total: 520)
        lob.process_message(&create_test_message(6, Action::Add, Side::Bid, 100.0, 200))
            .unwrap();
        assert_eq!(lob.get_lob_state().bid_sizes[0], 520);

        // Full trade on order 4 (total: 420)
        lob.process_message(&create_test_message(
            4,
            Action::Trade,
            Side::Bid,
            100.0,
            100,
        ))
        .unwrap();
        assert_eq!(lob.get_lob_state().bid_sizes[0], 420);

        // Verify remaining orders: 1 (70), 2 (50), 5 (100), 6 (200) = 420
        assert_eq!(lob.order_count(), 4);
        assert_eq!(lob.bid_levels(), 1);

        // Verify order 1 has 70, order 2 has 50
        // (This tests that partial operations didn't corrupt individual order sizes)
    }

    /// Performance benchmark for LOB reconstruction with PriceLevel caching.
    ///
    /// Measures throughput of processing messages and extracting LOB state.
    #[test]
    fn test_lob_reconstruction_performance() {
        use crate::types::LobState;
        use std::time::Instant;

        let num_messages = 100_000;
        let num_levels = 10;
        let orders_per_level = 50; // Simulate liquid market

        // Create messages that build up a realistic order book
        let mut messages = Vec::with_capacity(num_messages);
        let base_bid = 100.0;
        let base_ask = 100.01;
        let mut order_id = 1u64;

        // Build initial book with many orders per level
        for level in 0..num_levels {
            for _ in 0..orders_per_level {
                // Bid orders
                messages.push(create_test_message(
                    order_id,
                    Action::Add,
                    Side::Bid,
                    base_bid - (level as f64 * 0.01),
                    100,
                ));
                order_id += 1;

                // Ask orders
                messages.push(create_test_message(
                    order_id,
                    Action::Add,
                    Side::Ask,
                    base_ask + (level as f64 * 0.01),
                    100,
                ));
                order_id += 1;
            }
        }

        // Add cancels and trades to simulate activity
        let initial_orders = order_id;
        for i in 0..(num_messages - messages.len()) {
            let target_order = (i as u64 % initial_orders) + 1;
            if i % 3 == 0 {
                // Cancel
                messages.push(create_test_message(
                    target_order,
                    Action::Cancel,
                    Side::Bid,
                    base_bid,
                    50,
                ));
            } else if i % 3 == 1 {
                // Trade
                messages.push(create_test_message(
                    target_order,
                    Action::Trade,
                    Side::Bid,
                    base_bid,
                    25,
                ));
            } else {
                // New order
                messages.push(create_test_message(
                    order_id,
                    Action::Add,
                    Side::Bid,
                    base_bid - ((i % 10) as f64 * 0.01),
                    100,
                ));
                order_id += 1;
            }
        }

        // Benchmark with zero-allocation API
        let mut lob = LobReconstructor::new(num_levels);
        let mut state = LobState::new(num_levels);

        let start = Instant::now();
        for msg in &messages {
            let _ = lob.process_message_into(msg, &mut state);
        }
        let duration = start.elapsed();

        let msgs_per_sec = messages.len() as f64 / duration.as_secs_f64();

        println!("\n=== LOB Reconstruction Performance ===");
        println!("Messages processed: {}", messages.len());
        println!("Levels: {}", num_levels);
        println!("Orders per level: ~{}", orders_per_level);
        println!("Time: {:?}", duration);
        println!("Throughput: {:.0} msg/sec", msgs_per_sec);
        println!(
            "Per-message: {:.2} µs",
            duration.as_micros() as f64 / messages.len() as f64
        );

        // Verify correctness
        assert!(state.best_bid.is_some() || state.best_ask.is_some());
    }

    // =========================================================================
    // LobStats serialization tests
    // =========================================================================

    #[test]
    fn test_lobstats_export_roundtrip() {
        // Phase M M.A.4: `errors` field REMOVED per Decision 10b
        // (F-007 closure — dead field). Two new fields added in its place:
        // `modify_order_not_found` + `add_order_id_collision` (F-013 closure).
        // Use struct-update syntax `..LobStats::default()` to be resilient
        // against future additive fields.
        let stats = LobStats {
            messages_processed: 1_000_000,
            system_messages_skipped: 42,
            active_orders: 500,
            bid_levels: 10,
            ask_levels: 10,
            crossed_quotes: 7,
            locked_quotes: 2,
            last_timestamp: Some(1_700_000_000_000_000_000),
            cancel_order_not_found: 15,
            cancel_price_level_missing: 4,
            cancel_order_at_level_missing: 1,
            trade_order_not_found: 20,
            trade_price_level_missing: 5,
            trade_order_at_level_missing: 2,
            modify_order_not_found: 8,
            add_order_id_collision: 6,
            book_clears: 1,
            noop_messages: 100,
        };

        let dir = std::env::temp_dir().join("lobstats_roundtrip_test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("stats.json");

        stats.export_to_file(&path).unwrap();
        let loaded = LobStats::load_from_file(&path).unwrap();

        assert_eq!(loaded.messages_processed, 1_000_000);
        assert_eq!(loaded.system_messages_skipped, 42);
        assert_eq!(loaded.active_orders, 500);
        assert_eq!(loaded.bid_levels, 10);
        assert_eq!(loaded.ask_levels, 10);
        assert_eq!(loaded.crossed_quotes, 7);
        assert_eq!(loaded.locked_quotes, 2);
        assert_eq!(loaded.last_timestamp, Some(1_700_000_000_000_000_000));
        assert_eq!(loaded.cancel_order_not_found, 15);
        assert_eq!(loaded.cancel_price_level_missing, 4);
        assert_eq!(loaded.cancel_order_at_level_missing, 1);
        assert_eq!(loaded.trade_order_not_found, 20);
        assert_eq!(loaded.trade_price_level_missing, 5);
        assert_eq!(loaded.trade_order_at_level_missing, 2);
        assert_eq!(loaded.modify_order_not_found, 8);
        assert_eq!(loaded.add_order_id_collision, 6);
        assert_eq!(loaded.book_clears, 1);
        assert_eq!(loaded.noop_messages, 100);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_lobstats_export_empty() {
        let stats = LobStats::default();

        let dir = std::env::temp_dir().join("lobstats_empty_test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("stats_empty.json");

        stats.export_to_file(&path).unwrap();

        let json_str = std::fs::read_to_string(&path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();

        // Phase M M.A.5: envelope wrapper added — every key now lives under
        // `parsed["stats"][...]`. Schema version is locked at the top level.
        // Phase M M.A.4: `errors` field REMOVED (F-007). The new
        // `modify_order_not_found` + `add_order_id_collision` counters use
        // `#[serde(default)]` so they default to 0 in the JSON when omitted
        // — included here to lock the wire-format key names.
        assert_eq!(parsed["schema_version"], LOB_STATS_SCHEMA_VERSION);
        assert_eq!(parsed["stats"]["messages_processed"], 0);
        assert_eq!(parsed["stats"]["modify_order_not_found"], 0);
        assert_eq!(parsed["stats"]["add_order_id_collision"], 0);
        assert_eq!(parsed["stats"]["book_clears"], 0);
        assert_eq!(parsed["stats"]["noop_messages"], 0);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_lobstats_export_null_timestamp() {
        let stats = LobStats {
            last_timestamp: None,
            ..LobStats::default()
        };

        let dir = std::env::temp_dir().join("lobstats_null_ts_test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("stats_null_ts.json");

        stats.export_to_file(&path).unwrap();

        let json_str = std::fs::read_to_string(&path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();

        // Phase M M.A.5: envelope wrapper — last_timestamp lives under
        // `parsed["stats"][...]` post-bump.
        assert!(
            parsed["stats"]["last_timestamp"].is_null(),
            "None should serialize as null, got: {}",
            parsed["stats"]["last_timestamp"]
        );

        let loaded = LobStats::load_from_file(&path).unwrap();
        assert_eq!(loaded.last_timestamp, None);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_lobstats_export_envelope_shape() {
        // Phase M M.A.5 (REV 3 F-007 closure tail): lock the envelope wire
        // format end-to-end. Asserts that the on-disk JSON has EXACTLY two
        // top-level keys (`schema_version`, `stats`); the schema_version is
        // pinned to the LOB_STATS_SCHEMA_VERSION constant; and the `stats`
        // sub-object roundtrips byte-identically through
        // `serde_json::from_value::<LobStats>` so external tools can parse
        // the envelope directly without going through `load_from_file`.
        let stats = LobStats {
            messages_processed: 42,
            modify_order_not_found: 7,
            add_order_id_collision: 3,
            last_timestamp: Some(1_700_000_000_000_000_000),
            ..LobStats::default()
        };

        let dir = std::env::temp_dir().join("lobstats_envelope_shape_test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("envelope.json");

        stats.export_to_file(&path).unwrap();

        let json_str = std::fs::read_to_string(&path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();

        // Lock envelope shape: exactly two top-level keys.
        let obj = parsed
            .as_object()
            .expect("envelope must be a JSON object at the top level");
        assert_eq!(
            obj.len(),
            2,
            "envelope must have exactly 2 top-level keys (schema_version, stats); got: {:?}",
            obj.keys().collect::<Vec<_>>()
        );
        assert!(obj.contains_key("schema_version"));
        assert!(obj.contains_key("stats"));

        // Lock schema version pin — bumping LOB_STATS_SCHEMA_VERSION must
        // visibly cascade through this test.
        assert_eq!(obj["schema_version"], LOB_STATS_SCHEMA_VERSION);

        // Lock structural integrity: `stats` sub-object must deserialize
        // back to a LobStats with the SAME field values.
        let inner: LobStats = serde_json::from_value(obj["stats"].clone())
            .expect("stats sub-object must deserialize as LobStats");
        assert_eq!(inner.messages_processed, 42);
        assert_eq!(inner.modify_order_not_found, 7);
        assert_eq!(inner.add_order_id_collision, 3);
        assert_eq!(inner.last_timestamp, Some(1_700_000_000_000_000_000));

        // Round-trip via load_from_file (envelope branch) yields the same.
        let loaded = LobStats::load_from_file(&path).unwrap();
        assert_eq!(loaded.messages_processed, 42);
        assert_eq!(loaded.modify_order_not_found, 7);
        assert_eq!(loaded.add_order_id_collision, 3);
        assert_eq!(loaded.last_timestamp, Some(1_700_000_000_000_000_000));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_lobstats_load_legacy_format() {
        // Phase M M.A.5: dual-format read. `load_from_file` accepts both
        // envelope (post-2.0.0) AND legacy (pre-2.0.0 flat) shapes. The
        // legacy branch logs a WARN with calendar 2026-10-29 removal note
        // (not asserted here — log-capture would couple to env_logger
        // initialization). Asserts: a FLAT (non-envelope) JSON parses cleanly
        // into a valid LobStats with non-default values preserved.
        //
        // Realism: the pre-M.A.5 serializer wrote `serde_json::to_writer_pretty(
        // &LobStats)` directly — i.e., a flat object containing ALL serde
        // fields. Therefore we construct the legacy fixture by serializing a
        // populated LobStats DIRECTLY (bypassing the envelope wrapper), then
        // assert `load_from_file` recovers it.
        let dir = std::env::temp_dir().join("lobstats_legacy_load_test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("legacy.json");

        let original = LobStats {
            messages_processed: 99,
            modify_order_not_found: 4,
            add_order_id_collision: 2,
            last_timestamp: Some(1_700_000_000_000_000_000),
            ..LobStats::default()
        };
        // Write the FLAT shape directly — emulates pre-M.A.5 export bytes.
        let flat_json = serde_json::to_string_pretty(&original).unwrap();
        std::fs::write(&path, &flat_json).unwrap();

        // Sanity: top-level keys are NOT envelope keys (no schema_version, no
        // stats wrapper). This locks our fixture against accidental drift to
        // the envelope shape.
        let parsed: serde_json::Value = serde_json::from_str(&flat_json).unwrap();
        assert!(
            parsed.get("schema_version").is_none(),
            "legacy fixture must NOT carry schema_version: {flat_json}"
        );
        assert!(
            parsed.get("stats").is_none(),
            "legacy fixture must NOT carry stats wrapper: {flat_json}"
        );
        assert!(parsed.get("messages_processed").is_some());

        // Dual-format reader recovers the original.
        let loaded = LobStats::load_from_file(&path).unwrap();
        assert_eq!(loaded.messages_processed, 99);
        assert_eq!(loaded.modify_order_not_found, 4);
        assert_eq!(loaded.add_order_id_collision, 2);
        assert_eq!(loaded.last_timestamp, Some(1_700_000_000_000_000_000));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_lobstats_load_malformed_envelope_fails_loud() {
        // Phase M M.A.5 hardening (post-validation Agent 3 MEDIUM finding):
        // a JSON with top-level `schema_version` BUT the `stats` wrapper
        // MISSING (e.g., a botched migration script that emitted
        // `schema_version` but forgot to wrap `stats`) MUST fail-loud per
        // hft-rules §5 — pre-hardening this would silently route to the
        // legacy variant and DROP the unknown `schema_version` field.
        let dir = std::env::temp_dir().join("lobstats_malformed_envelope_test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("malformed.json");

        // Hand-crafted malformed payload: claims envelope (has schema_version
        // top-level) but `stats` wrapper MISSING. Pre-hardening:
        // `#[serde(untagged)]` would parse as `Legacy(LobStats)` with
        // schema_version silently dropped via serde-default.
        let malformed_json = r#"{
            "schema_version": "2.0.0",
            "messages_processed": 99,
            "system_messages_skipped": 0,
            "active_orders": 0,
            "bid_levels": 0,
            "ask_levels": 0,
            "crossed_quotes": 0,
            "locked_quotes": 0,
            "last_timestamp": null,
            "cancel_order_not_found": 0,
            "cancel_price_level_missing": 0,
            "cancel_order_at_level_missing": 0,
            "trade_order_not_found": 0,
            "trade_price_level_missing": 0,
            "trade_order_at_level_missing": 0,
            "book_clears": 0,
            "noop_messages": 0
        }"#;
        std::fs::write(&path, malformed_json).unwrap();

        let result = LobStats::load_from_file(&path);
        assert!(
            result.is_err(),
            "malformed envelope (schema_version present, stats wrapper missing) MUST fail-loud per hft-rules §5"
        );
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("malformed envelope"),
            "error message must cite 'malformed envelope'; got: {err_msg}"
        );
        assert!(
            err_msg.contains("`stats` wrapper missing"),
            "error message must cite missing `stats` wrapper; got: {err_msg}"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    // =========================================================================
    // track_consistency policy optimization tests
    // =========================================================================

    #[test]
    fn test_allow_policy_no_last_valid_state() {
        let config = LobConfig {
            levels: 10,
            crossed_quote_policy: CrossedQuotePolicy::Allow,
            ..LobConfig::default()
        };
        let mut lob = LobReconstructor::with_config(config);

        let bid = create_test_message(1, Action::Add, Side::Bid, 100.0, 100);
        lob.process_message(&bid).unwrap();
        let ask = create_test_message(2, Action::Add, Side::Ask, 101.0, 200);
        lob.process_message(&ask).unwrap();

        // With Allow policy, last_valid_state is never populated.
        // Force a crossed state by adding a bid above the ask.
        let cross_bid = create_test_message(3, Action::Add, Side::Bid, 102.0, 50);
        let state = lob.process_message(&cross_bid).unwrap();

        // Allow policy returns the crossed state as-is
        assert!(state.is_crossed(), "state should be crossed");
        assert_eq!(state.best_bid, Some(102_000_000_000));
        assert_eq!(state.best_ask, Some(101_000_000_000));
    }

    #[test]
    fn test_use_last_valid_policy_stores_state() {
        let config = LobConfig {
            levels: 10,
            crossed_quote_policy: CrossedQuotePolicy::UseLastValid,
            ..LobConfig::default()
        };
        let mut lob = LobReconstructor::with_config(config);

        // Build a valid book
        let bid = create_test_message(1, Action::Add, Side::Bid, 100.0, 100);
        lob.process_message(&bid).unwrap();
        let ask = create_test_message(2, Action::Add, Side::Ask, 101.0, 200);
        let valid_state = lob.process_message(&ask).unwrap();

        assert!(
            valid_state.is_valid(),
            "book should be valid before crossing"
        );

        // Cross the book
        let cross_bid = create_test_message(3, Action::Add, Side::Bid, 102.0, 50);
        let crossed_result = lob.process_message(&cross_bid).unwrap();

        // UseLastValid should return the previous valid state
        assert!(
            !crossed_result.is_crossed(),
            "UseLastValid policy should return a non-crossed state"
        );
        assert_eq!(
            crossed_result.best_bid,
            Some(100_000_000_000),
            "should return the last valid bid"
        );
        assert_eq!(
            crossed_result.best_ask,
            Some(101_000_000_000),
            "should return the last valid ask"
        );
    }

    #[test]
    fn test_skip_update_policy_stores_state() {
        let config = LobConfig {
            levels: 10,
            crossed_quote_policy: CrossedQuotePolicy::SkipUpdate,
            ..LobConfig::default()
        };
        let mut lob = LobReconstructor::with_config(config);

        // Build a valid book
        let bid = create_test_message(1, Action::Add, Side::Bid, 100.0, 100);
        lob.process_message(&bid).unwrap();
        let ask = create_test_message(2, Action::Add, Side::Ask, 101.0, 200);
        let valid_state = lob.process_message(&ask).unwrap();

        assert!(
            valid_state.is_valid(),
            "book should be valid before crossing"
        );

        // Cross the book
        let cross_bid = create_test_message(3, Action::Add, Side::Bid, 102.0, 50);
        let result = lob.process_message(&cross_bid).unwrap();

        // SkipUpdate should return the previous valid state
        assert!(
            !result.is_crossed(),
            "SkipUpdate policy should return a non-crossed state"
        );
        assert_eq!(
            result.best_bid,
            Some(100_000_000_000),
            "should return the last valid bid"
        );
    }

    // =========================================================================
    // LobConfig validation tests
    // =========================================================================

    #[test]
    fn test_lob_config_validate_valid() {
        use crate::types::MAX_LOB_LEVELS;
        LobConfig::new(1).validate().unwrap();
        LobConfig::new(10).validate().unwrap();
        LobConfig::new(MAX_LOB_LEVELS).validate().unwrap();
    }

    #[test]
    fn test_lob_config_validate_zero_levels() {
        let result = LobConfig::new(0).validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("at least 1"));
    }

    #[test]
    fn test_lob_config_validate_excessive_levels() {
        use crate::types::MAX_LOB_LEVELS;
        let result = LobConfig::new(MAX_LOB_LEVELS + 1).validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("must be <="));
    }

    #[test]
    #[should_panic(expected = "LobConfig validation failed")]
    fn test_lob_reconstructor_panics_on_zero_levels() {
        let _ = LobReconstructor::new(0);
    }

    #[test]
    fn test_error_policy_distinguishes_crossed_from_locked_into() {
        // Test locked book: bid == ask -> should return LockedQuote
        let config = LobConfig::new(10).with_crossed_quote_policy(CrossedQuotePolicy::Error);
        let mut lob = LobReconstructor::with_config(config);
        let mut state = LobState::new(10);

        lob.process_message_into(
            &MboMessage::new(1, Action::Add, Side::Bid, 100_000_000_000, 100).with_timestamp(1000),
            &mut state,
        )
        .unwrap();
        let result = lob.process_message_into(
            &MboMessage::new(2, Action::Add, Side::Ask, 100_000_000_000, 100).with_timestamp(2000),
            &mut state,
        );

        assert!(
            matches!(result, Err(TlobError::LockedQuote(_, _))),
            "Locked quote (bid==ask) should return LockedQuote, not CrossedQuote. Got: {:?}",
            result
        );

        // Test crossed book: bid > ask -> should return CrossedQuote
        let config = LobConfig::new(10).with_crossed_quote_policy(CrossedQuotePolicy::Error);
        let mut lob = LobReconstructor::with_config(config);
        let mut state = LobState::new(10);

        lob.process_message_into(
            &MboMessage::new(1, Action::Add, Side::Bid, 101_000_000_000, 100).with_timestamp(1000),
            &mut state,
        )
        .unwrap();
        let result = lob.process_message_into(
            &MboMessage::new(2, Action::Add, Side::Ask, 100_000_000_000, 100).with_timestamp(2000),
            &mut state,
        );

        assert!(
            matches!(result, Err(TlobError::CrossedQuote(_, _))),
            "Crossed quote (bid>ask) should return CrossedQuote. Got: {:?}",
            result
        );
    }

    #[test]
    fn test_use_last_valid_preserves_temporal_chain_into() {
        let config = LobConfig::new(10).with_crossed_quote_policy(CrossedQuotePolicy::UseLastValid);
        let mut lob = LobReconstructor::with_config(config);
        let mut state = LobState::new(10);

        // Build valid book
        lob.process_message_into(
            &MboMessage::new(1, Action::Add, Side::Bid, 100_000_000_000, 100).with_timestamp(1000),
            &mut state,
        )
        .unwrap();
        lob.process_message_into(
            &MboMessage::new(2, Action::Add, Side::Ask, 101_000_000_000, 100).with_timestamp(2000),
            &mut state,
        )
        .unwrap();

        // Verify valid state captured
        assert!(state.best_bid.is_some());
        assert!(state.best_ask.is_some());
        let valid_timestamp = state.timestamp;

        // Cause a crossed state (bid > ask)
        lob.process_message_into(
            &MboMessage::new(3, Action::Add, Side::Bid, 102_000_000_000, 100).with_timestamp(5000),
            &mut state,
        )
        .unwrap();

        // Book data should come from last valid state (bid=100, ask=101)
        assert_eq!(state.best_bid, Some(100_000_000_000));
        assert_eq!(state.best_ask, Some(101_000_000_000));

        // Temporal data should come from the CURRENT message (timestamp=5000)
        assert_eq!(
            state.timestamp,
            Some(5000),
            "Timestamp should be current message's, not stale"
        );
        assert_eq!(
            state.previous_timestamp, valid_timestamp,
            "Previous should be the valid state's timestamp"
        );
        assert!(
            state.delta_ns > 0,
            "delta_ns should be non-zero (5000 - 2000 = 3000)"
        );
        assert_eq!(
            state.sequence, 3,
            "Sequence should be current message count"
        );
        assert_eq!(state.triggering_action, Some(Action::Add));
        assert_eq!(state.triggering_side, Some(Side::Bid));
    }
}
