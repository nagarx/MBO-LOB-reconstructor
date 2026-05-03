//! Column-oriented batch buffers for LOB and MBO Parquet export.
//!
//! Pre-allocates `Vec` columns at `batch_size` capacity, appends row-by-row,
//! then converts to an Arrow `RecordBatch` for flushing. After flush, the
//! vectors are cleared (but retain their allocation) for reuse.

use std::sync::Arc;

use arrow::array::{
    ArrayRef, FixedSizeListArray, Float64Array, Int64Array, RecordBatch, UInt32Array, UInt64Array,
    UInt8Array,
};
use arrow::datatypes::Schema;

use crate::error::{Result, TlobError};
use crate::types::{LobState, MboMessage, MAX_LOB_LEVELS};

use super::schema::book_consistency_to_u8;

// ─────────────────────────────────────────────────────────────────────────────
// LOB snapshot batch
// ─────────────────────────────────────────────────────────────────────────────

/// Column-oriented buffer for LOB snapshot rows.
pub(crate) struct LobBatch {
    levels: usize,
    include_derived: bool,
    capacity: usize,

    // Core columns
    pub(crate) timestamp_ns: Vec<i64>,
    pub(crate) sequence: Vec<u64>,
    pub(crate) level_count: Vec<u8>,
    pub(crate) best_bid: Vec<Option<i64>>,
    pub(crate) best_ask: Vec<Option<i64>>,
    pub(crate) bid_prices: Vec<i64>,
    pub(crate) bid_sizes: Vec<u32>,
    pub(crate) ask_prices: Vec<i64>,
    pub(crate) ask_sizes: Vec<u32>,
    pub(crate) delta_ns: Vec<u64>,
    pub(crate) triggering_action: Vec<Option<u8>>,
    pub(crate) triggering_side: Vec<Option<u8>>,

    // Derived columns (populated only when include_derived is true)
    pub(crate) mid_price: Vec<Option<f64>>,
    pub(crate) spread: Vec<Option<f64>>,
    pub(crate) spread_bps: Vec<Option<f64>>,
    pub(crate) microprice: Vec<Option<f64>>,
    pub(crate) total_bid_volume: Vec<u64>,
    pub(crate) total_ask_volume: Vec<u64>,
    pub(crate) depth_imbalance: Vec<Option<f64>>,
    pub(crate) book_consistency: Vec<u8>,
}

impl LobBatch {
    pub(crate) fn new(capacity: usize, levels: usize, include_derived: bool) -> Self {
        Self {
            levels,
            include_derived,
            capacity,
            timestamp_ns: Vec::with_capacity(capacity),
            sequence: Vec::with_capacity(capacity),
            level_count: Vec::with_capacity(capacity),
            best_bid: Vec::with_capacity(capacity),
            best_ask: Vec::with_capacity(capacity),
            bid_prices: Vec::with_capacity(capacity * levels),
            bid_sizes: Vec::with_capacity(capacity * levels),
            ask_prices: Vec::with_capacity(capacity * levels),
            ask_sizes: Vec::with_capacity(capacity * levels),
            delta_ns: Vec::with_capacity(capacity),
            triggering_action: Vec::with_capacity(capacity),
            triggering_side: Vec::with_capacity(capacity),
            mid_price: Vec::with_capacity(if include_derived { capacity } else { 0 }),
            spread: Vec::with_capacity(if include_derived { capacity } else { 0 }),
            spread_bps: Vec::with_capacity(if include_derived { capacity } else { 0 }),
            microprice: Vec::with_capacity(if include_derived { capacity } else { 0 }),
            total_bid_volume: Vec::with_capacity(if include_derived { capacity } else { 0 }),
            total_ask_volume: Vec::with_capacity(if include_derived { capacity } else { 0 }),
            depth_imbalance: Vec::with_capacity(if include_derived { capacity } else { 0 }),
            book_consistency: Vec::with_capacity(if include_derived { capacity } else { 0 }),
        }
    }

    /// Append a single `LobState` snapshot to the batch.
    ///
    /// # Phase O Cycle 1 / B.1 — single source of truth for levels
    ///
    /// Pre-B.1, this method had a dual source of truth: `level_count` was
    /// pushed from `state.levels` while iteration used `self.levels`. When
    /// the two differed (a misconfiguration where `LobReconstructor::new(N)`
    /// is paired with a `LobSnapshotWriter` configured for `M ≠ N`), the
    /// on-disk row would record one count but materialize a different
    /// number of FixedSizeList values. That broke the per-file `levels`
    /// schema invariant that downstream Python consumers rely on (e.g.,
    /// `MBO-LOB-analyzer/src/rawlobanalyzer/io/schema.py:80` hardcodes
    /// `pa.list_(pa.int64(), 10)`).
    ///
    /// Post-B.1: `self.levels` is the SSoT for BOTH the `level_count`
    /// column AND the iteration bound. The on-disk `levels` column is a
    /// per-file constant equal to `ExportConfig.levels` for every row.
    ///
    /// # Invariants enforced (debug builds)
    ///
    /// - `state.levels == self.levels`: in production this is guaranteed
    ///   by `LobReconstructor::fill_lob_state_with_temporal` at
    ///   `reconstructor.rs:1075` setting `state.levels = self.config.levels`.
    ///   `debug_assert_eq!` here catches misconfiguration at development
    ///   time + external direct mutation of `pub state.levels`
    ///   (`types.rs:309`).
    /// - `self.levels <= MAX_LOB_LEVELS`: the caller (`LobBatch::new` via
    ///   `ExportConfig::validate`) already enforces this bound; the
    ///   `debug_assert!` documents it at the use site (defense-in-depth).
    ///
    /// In release builds (debug_assert compiled out), the `level_count`
    /// column STILL records `self.levels` (not `state.levels`), preserving
    /// the per-file constant contract even under misconfiguration. The
    /// for-loop bound is `self.levels`, so `state.bid_prices[i]` is
    /// always indexed within `[0, self.levels)`. Stack-allocated
    /// `bid_prices: [i64; MAX_LOB_LEVELS]` is bounds-safe iff
    /// `self.levels <= MAX_LOB_LEVELS`, which the construction-time
    /// `ExportConfig::validate` enforces. No release-mode UB.
    pub(crate) fn push(&mut self, state: &LobState) {
        debug_assert!(
            self.levels <= MAX_LOB_LEVELS,
            "LobBatch::push: self.levels ({}) > MAX_LOB_LEVELS ({}); \
             ExportConfig::validate should have caught this at construction",
            self.levels,
            MAX_LOB_LEVELS,
        );
        debug_assert_eq!(
            state.levels, self.levels,
            "LobBatch::push: state.levels ({}) != writer self.levels ({}); \
             caller must construct LobReconstructor and LobSnapshotWriter with \
             identical `levels` (production code path via \
             `process_message_into` -> `fill_lob_state_with_temporal` at \
             reconstructor.rs:1075 sets state.levels = config.levels — this \
             panic indicates either (a) external mutation of pub state.levels \
             (types.rs:309) or (b) reconstructor/writer levels misconfiguration)",
            state.levels, self.levels,
        );

        self.timestamp_ns.push(state.timestamp.unwrap_or(0));
        self.sequence.push(state.sequence);
        // B.1: SSoT for the per-file `levels` column is self.levels (=
        // ExportConfig.levels). Pre-B.1 this was state.levels, which could
        // diverge from the FixedSizeList column width under misconfiguration.
        self.level_count.push(self.levels as u8);
        self.best_bid.push(state.best_bid);
        self.best_ask.push(state.best_ask);

        for i in 0..self.levels {
            self.bid_prices.push(state.bid_prices[i]);
            self.bid_sizes.push(state.bid_sizes[i]);
            self.ask_prices.push(state.ask_prices[i]);
            self.ask_sizes.push(state.ask_sizes[i]);
        }

        self.delta_ns.push(state.delta_ns);
        self.triggering_action
            .push(state.triggering_action.map(|a| a.to_byte()));
        self.triggering_side
            .push(state.triggering_side.map(|s| s.to_byte()));

        if self.include_derived {
            self.mid_price.push(state.mid_price());
            self.spread.push(state.spread());
            self.spread_bps.push(state.spread_bps());
            self.microprice.push(state.microprice());
            self.total_bid_volume.push(state.total_bid_volume());
            self.total_ask_volume.push(state.total_ask_volume());
            self.depth_imbalance.push(state.depth_imbalance());
            self.book_consistency
                .push(book_consistency_to_u8(state.check_consistency()));
        }
    }

    /// Number of rows currently in the batch.
    pub(crate) fn len(&self) -> usize {
        self.timestamp_ns.len()
    }

    /// Whether the batch has reached capacity and should be flushed.
    pub(crate) fn is_full(&self) -> bool {
        self.len() >= self.capacity
    }

    /// Convert the buffered columns into an Arrow `RecordBatch`, then clear
    /// the buffers (retaining their heap allocations for reuse).
    pub(crate) fn flush(&mut self, schema: &Schema) -> Result<RecordBatch> {
        let n = self.levels as i32;
        let row_count = self.len();

        let mut columns: Vec<ArrayRef> = vec![
            Arc::new(Int64Array::from(std::mem::take(&mut self.timestamp_ns))),
            Arc::new(UInt64Array::from(std::mem::take(&mut self.sequence))),
            Arc::new(UInt8Array::from(std::mem::take(&mut self.level_count))),
            Arc::new(nullable_i64_array(std::mem::take(&mut self.best_bid))),
            Arc::new(nullable_i64_array(std::mem::take(&mut self.best_ask))),
            build_fixed_list_i64(std::mem::take(&mut self.bid_prices), n, row_count)?,
            build_fixed_list_u32(std::mem::take(&mut self.bid_sizes), n, row_count)?,
            build_fixed_list_i64(std::mem::take(&mut self.ask_prices), n, row_count)?,
            build_fixed_list_u32(std::mem::take(&mut self.ask_sizes), n, row_count)?,
            Arc::new(UInt64Array::from(std::mem::take(&mut self.delta_ns))),
            Arc::new(nullable_u8_array(std::mem::take(
                &mut self.triggering_action,
            ))),
            Arc::new(nullable_u8_array(std::mem::take(&mut self.triggering_side))),
        ];

        if self.include_derived {
            columns.extend([
                Arc::new(nullable_f64_array(std::mem::take(&mut self.mid_price))) as ArrayRef,
                Arc::new(nullable_f64_array(std::mem::take(&mut self.spread))),
                Arc::new(nullable_f64_array(std::mem::take(&mut self.spread_bps))),
                Arc::new(nullable_f64_array(std::mem::take(&mut self.microprice))),
                Arc::new(UInt64Array::from(std::mem::take(
                    &mut self.total_bid_volume,
                ))),
                Arc::new(UInt64Array::from(std::mem::take(
                    &mut self.total_ask_volume,
                ))),
                Arc::new(nullable_f64_array(std::mem::take(
                    &mut self.depth_imbalance,
                ))),
                Arc::new(UInt8Array::from(std::mem::take(&mut self.book_consistency))),
            ]);
        }

        // Re-initialize capacity for reuse
        self.reserve();

        RecordBatch::try_new(Arc::new(schema.clone()), columns)
            .map_err(|e| TlobError::Generic(format!("Failed to build RecordBatch: {e}")))
    }

    /// Re-allocate capacity after a flush (vectors were taken, need fresh capacity).
    fn reserve(&mut self) {
        let cap = self.capacity;
        let lcap = cap * self.levels;
        let dcap = if self.include_derived { cap } else { 0 };

        self.timestamp_ns.reserve(cap);
        self.sequence.reserve(cap);
        self.level_count.reserve(cap);
        self.best_bid.reserve(cap);
        self.best_ask.reserve(cap);
        self.bid_prices.reserve(lcap);
        self.bid_sizes.reserve(lcap);
        self.ask_prices.reserve(lcap);
        self.ask_sizes.reserve(lcap);
        self.delta_ns.reserve(cap);
        self.triggering_action.reserve(cap);
        self.triggering_side.reserve(cap);
        self.mid_price.reserve(dcap);
        self.spread.reserve(dcap);
        self.spread_bps.reserve(dcap);
        self.microprice.reserve(dcap);
        self.total_bid_volume.reserve(dcap);
        self.total_ask_volume.reserve(dcap);
        self.depth_imbalance.reserve(dcap);
        self.book_consistency.reserve(dcap);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// MBO event batch
// ─────────────────────────────────────────────────────────────────────────────

/// Column-oriented buffer for MBO event rows.
pub(crate) struct MboBatch {
    capacity: usize,

    pub(crate) timestamp_ns: Vec<Option<i64>>,
    pub(crate) order_id: Vec<u64>,
    pub(crate) action: Vec<u8>,
    pub(crate) side: Vec<u8>,
    pub(crate) price: Vec<i64>,
    pub(crate) size: Vec<u32>,
}

impl MboBatch {
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            capacity,
            timestamp_ns: Vec::with_capacity(capacity),
            order_id: Vec::with_capacity(capacity),
            action: Vec::with_capacity(capacity),
            side: Vec::with_capacity(capacity),
            price: Vec::with_capacity(capacity),
            size: Vec::with_capacity(capacity),
        }
    }

    pub(crate) fn push(&mut self, msg: &MboMessage) {
        self.timestamp_ns.push(msg.timestamp);
        self.order_id.push(msg.order_id);
        self.action.push(msg.action.to_byte());
        self.side.push(msg.side.to_byte());
        self.price.push(msg.price);
        self.size.push(msg.size);
    }

    pub(crate) fn len(&self) -> usize {
        self.timestamp_ns.len()
    }

    pub(crate) fn is_full(&self) -> bool {
        self.len() >= self.capacity
    }

    pub(crate) fn flush(&mut self, schema: &Schema) -> Result<RecordBatch> {
        let columns: Vec<ArrayRef> = vec![
            Arc::new(nullable_i64_array(std::mem::take(&mut self.timestamp_ns))),
            Arc::new(UInt64Array::from(std::mem::take(&mut self.order_id))),
            Arc::new(UInt8Array::from(std::mem::take(&mut self.action))),
            Arc::new(UInt8Array::from(std::mem::take(&mut self.side))),
            Arc::new(Int64Array::from(std::mem::take(&mut self.price))),
            Arc::new(UInt32Array::from(std::mem::take(&mut self.size))),
        ];

        let cap = self.capacity;
        self.timestamp_ns.reserve(cap);
        self.order_id.reserve(cap);
        self.action.reserve(cap);
        self.side.reserve(cap);
        self.price.reserve(cap);
        self.size.reserve(cap);

        RecordBatch::try_new(Arc::new(schema.clone()), columns)
            .map_err(|e| TlobError::Generic(format!("Failed to build MBO RecordBatch: {e}")))
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Arrow array helpers
// ─────────────────────────────────────────────────────────────────────────────

fn nullable_i64_array(values: Vec<Option<i64>>) -> Int64Array {
    Int64Array::from(values)
}

fn nullable_u8_array(values: Vec<Option<u8>>) -> UInt8Array {
    UInt8Array::from(values)
}

fn nullable_f64_array(values: Vec<Option<f64>>) -> Float64Array {
    Float64Array::from(values)
}

/// Build a `FixedSizeList<Int64>` from a flat vector of values.
///
/// `flat` has `row_count * list_size` elements, laid out row-major.
fn build_fixed_list_i64(flat: Vec<i64>, list_size: i32, row_count: usize) -> Result<ArrayRef> {
    debug_assert_eq!(
        flat.len(),
        row_count * list_size as usize,
        "flat i64 length mismatch"
    );
    let values = Arc::new(Int64Array::from(flat));
    let field = Arc::new(arrow::datatypes::Field::new(
        "item",
        arrow::datatypes::DataType::Int64,
        false,
    ));
    FixedSizeListArray::try_new(field, list_size, values, None)
        .map(|a| Arc::new(a) as ArrayRef)
        .map_err(|e| TlobError::Generic(format!("FixedSizeList<Int64> error: {e}")))
}

/// Build a `FixedSizeList<UInt32>` from a flat vector of values.
fn build_fixed_list_u32(flat: Vec<u32>, list_size: i32, row_count: usize) -> Result<ArrayRef> {
    debug_assert_eq!(
        flat.len(),
        row_count * list_size as usize,
        "flat u32 length mismatch"
    );
    let values = Arc::new(UInt32Array::from(flat));
    let field = Arc::new(arrow::datatypes::Field::new(
        "item",
        arrow::datatypes::DataType::UInt32,
        false,
    ));
    FixedSizeListArray::try_new(field, list_size, values, None)
        .map(|a| Arc::new(a) as ArrayRef)
        .map_err(|e| TlobError::Generic(format!("FixedSizeList<UInt32> error: {e}")))
}

// ─────────────────────────────────────────────────────────────────────────────
// Phase O Cycle 1 / B.1 — LobBatch level-source-of-truth regression tests
// ─────────────────────────────────────────────────────────────────────────────
//
// Per Phase O design (PHASE_O_DESIGN_REFINED_2026_05_03.md, B.1):
// pre-fix `LobBatch::push` had a dual source of truth — `level_count` was
// pushed from `state.levels` (line 88) while iteration used `self.levels`
// (line 92). When `state.levels != self.levels` (misconfiguration where a
// `LobReconstructor::new(N)` is paired with a `LobSnapshotWriter` configured
// for `M ≠ N`), the on-disk row would record one count but materialize a
// different number of FixedSizeList values, breaking the per-file
// `levels` schema invariant (which Python consumers like
// `MBO-LOB-analyzer/src/rawlobanalyzer/io/schema.py:80` rely on as a
// per-file constant equal to `ExportConfig.levels`).
//
// Post-fix discipline:
//   * `self.levels` is the SSoT for BOTH the `level_count` column AND the
//     iteration bound — the on-disk `levels` column is a per-file constant
//     equal to `ExportConfig.levels` for every row in the file.
//   * `debug_assert_eq!(state.levels, self.levels, ...)` catches
//     misconfiguration at development time (cargo test default + debug
//     builds). Release builds tolerate a state.levels mismatch but the
//     emitted column count remains self.levels — the pre-existing
//     contract is preserved.
//   * `debug_assert!(self.levels <= MAX_LOB_LEVELS)` is a defense-in-depth
//     bound check; the caller (`LobBatch::new` via `ExportConfig::validate`)
//     already enforces this, but the assert documents it at the use site.
//
// In production today (LobReconstructor → LobSnapshotWriter via
// `process_message_into` → `fill_lob_state_with_temporal` at
// `reconstructor.rs:1075` setting `state.levels = self.config.levels`),
// `state.levels == self.levels` is invariant. The fix is purely DEFENSIVE
// against external direct mutation of `pub state.levels` (types.rs:309)
// and against misconfigured pairings where reconstructor and writer
// `levels` diverge.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{LobState, MAX_LOB_LEVELS};

    /// Baseline: state.levels == self.levels (the production case).
    /// Verifies no regression for the normal path: 5 real levels in,
    /// 5 real levels out, level_count == 5.
    #[test]
    fn b1_lob_batch_baseline_state_equals_self_levels() {
        let mut batch = LobBatch::new(10, 5, false);
        let mut state = LobState::new(5);
        for i in 0..5 {
            state.bid_prices[i] = 100 - i as i64;
            state.bid_sizes[i] = 10 + i as u32;
            state.ask_prices[i] = 200 + i as i64;
            state.ask_sizes[i] = 20 + i as u32;
        }
        batch.push(&state);

        assert_eq!(batch.bid_prices.len(), 5, "5 bid prices for 1 row");
        assert_eq!(batch.bid_prices, vec![100, 99, 98, 97, 96]);
        assert_eq!(batch.bid_sizes, vec![10, 11, 12, 13, 14]);
        assert_eq!(batch.ask_prices, vec![200, 201, 202, 203, 204]);
        assert_eq!(batch.ask_sizes, vec![20, 21, 22, 23, 24]);
        assert_eq!(
            batch.level_count[0], 5,
            "level_count is the per-file constant self.levels (= ExportConfig.levels)"
        );
    }

    /// Critical regression test: state.levels != self.levels MUST panic in
    /// debug builds (proves the fail-loud discipline introduced by the
    /// B.1 fix). Pre-B.1: this test would NOT panic (no debug_assert
    /// existed) — the should_panic gate would FAIL the test, exposing
    /// the missing safety invariant. Post-B.1: panic fires with both
    /// values cited, the test PASSES.
    #[test]
    #[should_panic(expected = "state.levels")]
    fn b1_lob_batch_levels_mismatch_panics_in_debug() {
        let mut batch = LobBatch::new(10, 5, false);
        let mut state = LobState::new(3);
        state.bid_prices[0] = 100;
        state.bid_prices[1] = 99;
        state.bid_prices[2] = 98;
        // state.levels (3) != self.levels (5) → debug_assert_eq! fires.
        batch.push(&state);
    }

    /// Boundary case: self.levels = MAX_LOB_LEVELS. Verifies the upper
    /// bound is reachable without spurious panic. The
    /// `debug_assert!(self.levels <= MAX_LOB_LEVELS)` invariant in push()
    /// is `<=` (not `<`), so MAX_LOB_LEVELS itself is valid.
    #[test]
    fn b1_lob_batch_max_levels_boundary_accepted() {
        let mut batch = LobBatch::new(2, MAX_LOB_LEVELS, false);
        let mut state = LobState::new(MAX_LOB_LEVELS);
        for i in 0..MAX_LOB_LEVELS {
            state.bid_prices[i] = (1000 + i) as i64;
            state.ask_prices[i] = (2000 + i) as i64;
        }
        batch.push(&state);

        assert_eq!(batch.bid_prices.len(), MAX_LOB_LEVELS);
        assert_eq!(batch.level_count[0] as usize, MAX_LOB_LEVELS);
        assert_eq!(batch.bid_prices[0], 1000);
        assert_eq!(
            batch.bid_prices[MAX_LOB_LEVELS - 1],
            1000 + (MAX_LOB_LEVELS - 1) as i64
        );
    }
}
