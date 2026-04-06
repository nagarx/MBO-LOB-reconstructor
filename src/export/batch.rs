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
use crate::types::{LobState, MboMessage};

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
    pub(crate) fn push(&mut self, state: &LobState) {
        self.timestamp_ns.push(state.timestamp.unwrap_or(0));
        self.sequence.push(state.sequence);
        self.level_count.push(state.levels as u8);
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
