# ARCHITECTURE.md -- MBO-LOB-Reconstructor

Primary technical reference for LLM coders. Every claim verified against source code as of 2026-04-04.

---

## 1. Overview

**mbo-lob-reconstructor** v0.1.0 (Rust edition 2021, MSRV 1.82) converts Market-By-Order (MBO) data streams into Limit Order Book (LOB) snapshots. It is the foundation of the HFT pipeline, consumed directly by `feature-extractor-MBO-LOB` (which depends on it via `.cargo/config.toml` path override).

The crate provides:

- **LOB reconstruction**: MBO messages (Add/Modify/Cancel/Trade) to aggregated price-level snapshots (`LobState`, ~560 bytes stack-allocated).
- **Composable trackers**: Four standalone modules (queue position, order lifecycle, trade aggregation, day boundary detection) that each consume `MboMessage` independently -- no dependency on `LobReconstructor`.
- **ML analytics**: Microprice, VWAP, depth imbalance, market impact simulation, running statistics (Welford's algorithm), normalization parameters.
- **Parquet export**: Raw LOB snapshots and MBO events to Apache Parquet with schema versioning.
- **Databento integration**: Streaming loader for `.dbn.zst` files with hot store management.

Performance: ~1M msgs/sec, ~1 us/msg latency. Primary bottleneck is single-threaded zstd decompression; pre-decompressing to a hot store yields ~5x speedup.

---

## 2. Feature Flags

Defined in `Cargo.toml`:

| Feature | Default | Gates | Dependencies |
|---------|---------|-------|--------------|
| `databento` | Yes | `loader.rs`, `hotstore.rs`, `dbn_bridge.rs`, `DbnSource` in `source.rs` | `dbn` (git tag v0.20.0), `zstd` 0.13 |
| `export` | No | `export/` module (mod.rs, schema.rs, lob_writer.rs, mbo_writer.rs, batch.rs) | `arrow` 55, `parquet` 55, `toml` 0.8 |

Build without Databento: `cargo build --no-default-features`
Build with export: `cargo build --features export`
Both CLIs require `databento`; `export_to_parquet` additionally requires `export`.

---

## 3. Data Flow

```
.dbn.zst (compressed Databento MBO)
    |
    v
DbnLoader (src/loader.rs)              -- Streaming reader, 1MB I/O buffer
    |                                      Auto-detects compression via DynDecoder
    |  (optional) HotStoreManager          Resolves to decompressed .dbn if available
    |             (src/hotstore.rs)
    v
DbnBridge::convert() (src/dbn_bridge.rs) -- dbn::MboMsg -> MboMessage (~40B copy)
    |
    v
MboMessage (src/types.rs)               -- Core event: order_id, action, side, price, size, timestamp
    |
    +--------> LobReconstructor (src/lob/reconstructor.rs)
    |              |
    |              v
    |          LobState (~560B stack-allocated)
    |              |
    |              +---> DayStats / NormalizationParams (src/statistics.rs)
    |              +---> DepthStats / MarketImpact / LiquidityMetrics (src/analytics.rs)
    |
    +--------> QueuePositionTracker (src/lob/queue_position.rs)
    |              |
    |              v
    |          QueuePositionInfo (position, volume_ahead, queue_length)
    |
    +--------> OrderLifecycleTracker (src/lob/order_lifecycle.rs)
    |              |
    |              v
    |          LifecycleEvent (Created | Modified | PartialFill | Completed)
    |
    +--------> TradeAggregator (src/lob/trade_aggregator.rs)
    |              |
    |              v
    |          Trade (price, size, aggressor_side, tick_direction)
    |
    +--------> DayBoundaryDetector (src/lob/day_boundary.rs)
    |              |
    |              v
    |          DayBoundary (gap_ns, day_index, previous_day_stats)
    |
    +--------> LobSnapshotWriter (src/export/lob_writer.rs)  [feature=export]
    |              |
    |              v
    |          LOB snapshots -> Parquet
    |
    +--------> MboEventWriter (src/export/mbo_writer.rs)     [feature=export]
                   |
                   v
               MBO events -> Parquet
```

All trackers are composable: each consumes `MboMessage` independently and can be used alongside or without `LobReconstructor`.

---

## 4. Module Map

### Core Library

| File | Lines | Purpose | Key Public Types |
|------|-------|---------|-----------------|
| `src/lib.rs` | 242 | Crate root; module declarations, feature gates, re-exports | -- |
| `src/types.rs` | 1377 | Core data types: messages, book state, enums | `MboMessage`, `LobState`, `Action`, `Side`, `BookConsistency`, `Order`, `MAX_LOB_LEVELS` |
| `src/constants.rs` | 104 | Named domain constants with citations | All 10 constants (see Section 12) |
| `src/error.rs` | 103 | Error types using `thiserror` | `TlobError` (12 variants), `Result<T>` |
| `src/source.rs` | 598 | Data source abstraction trait | `MarketDataSource` (trait), `SourceMetadata`, `VecSource`, `DbnSource` [databento] |
| `src/statistics.rs` | 826 | Running stats (Welford), day stats, normalization params | `RunningStats`, `DayStats`, `NormalizationParams` |
| `src/analytics.rs` | 566 | Advanced analytics: depth, impact, liquidity | `DepthStats`, `MarketImpact`, `LiquidityMetrics` |
| `src/warnings.rs` | 717 | Warning tracking with categories and deduplication | `WarningTracker`, `Warning`, `WarningCategory`, `WarningSummary`, `WarningTrackerConfig` |

### LOB Module (`src/lob/`)

| File | Lines | Purpose | Key Public Types |
|------|-------|---------|-----------------|
| `src/lob/mod.rs` | 100 | Module declarations and re-exports | -- |
| `src/lob/reconstructor.rs` | 2557 | Single-symbol LOB reconstruction engine | `LobReconstructor`, `LobConfig`, `LobStats`, `CrossedQuotePolicy` |
| `src/lob/price_level.rs` | 430 | Orders at a price with O(1) cached total_size | `PriceLevel` |
| `src/lob/queue_position.rs` | 1683 | FIFO queue position tracking | `QueuePositionTracker`, `QueuePositionConfig`, `QueuePositionInfo`, `PositionChange`, `PositionChangeReason`, `QueueStats` |
| `src/lob/order_lifecycle.rs` | 1637 | Order lifecycle Add->Modify->Cancel/Fill | `OrderLifecycleTracker`, `OrderLifecycleConfig`, `OrderLifecycle`, `LifecycleEvent`, `LifecycleStats`, `OrderOrigin`, `TerminalState`, `OrderModification`, `ActiveOrderFeatures`, `CompletionStats` |
| `src/lob/trade_aggregator.rs` | 943 | Fills to trades with aggressor detection | `TradeAggregator`, `TradeAggregatorConfig`, `Trade`, `Fill` |
| `src/lob/day_boundary.rs` | 614 | Trading day boundary detection | `DayBoundaryDetector`, `DayBoundaryConfig`, `DayBoundary`, `DayBoundaryStats` |
| `src/lob/multi_symbol.rs` | 385 | Multi-symbol LOB manager | `MultiSymbolLob` |

### Databento Integration (`databento` feature)

| File | Lines | Purpose | Key Public Types |
|------|-------|---------|-----------------|
| `src/dbn_bridge.rs` | 287 | `dbn::MboMsg` to `MboMessage` conversion | `DbnBridge` |
| `src/loader.rs` | 574 | Streaming DBN file reader with 1MB I/O buffer | `DbnLoader`, `MessageIterator`, `LoaderStats`, `IO_BUFFER_SIZE` |
| `src/hotstore.rs` | 857 | Decompressed file management for fast I/O | `HotStoreManager`, `HotStoreConfig` |

### Export System (`export` feature)

| File | Lines | Purpose | Key Public Types |
|------|-------|---------|-----------------|
| `src/export/mod.rs` | 183 | Export config, downsampling, stats | `ExportConfig`, `DownsampleConfig`, `DownsampleStrategy`, `ParquetExportStats`, `SCHEMA_VERSION` |
| `src/export/schema.rs` | 347 | Arrow schema definitions (SSoT for data contract) | `lob_snapshot_schema()`, `mbo_event_schema()`, `book_consistency_to_u8()` |
| `src/export/lob_writer.rs` | 260 | LOB snapshot Parquet writer | `LobSnapshotWriter` |
| `src/export/mbo_writer.rs` | 198 | MBO event Parquet writer | `MboEventWriter` |
| `src/export/batch.rs` | 327 | Column-oriented batch buffers for Arrow conversion | `LobBatch` (pub(crate)), `MboBatch` (pub(crate)) |

### CLI Binaries

| File | Lines | Purpose | Required Features |
|------|-------|---------|-------------------|
| `src/bin/export_to_parquet.rs` | 677 | Export LOB snapshots and MBO events to Parquet | `databento`, `export` |
| `src/bin/decompress_to_hot_store.rs` | 379 | Pre-decompress `.dbn.zst` files for faster I/O | `databento` |

**Total**: ~17,000 lines of Rust across 26 source files.

---

## 5. Core Types Reference

### MboMessage (`src/types.rs`)

```
pub struct MboMessage {
    pub order_id: u64,              // Unique order identifier
    pub action: Action,             // What happened (Add, Modify, Cancel, Trade, Fill, Clear, None)
    pub side: Side,                 // Bid, Ask, or None
    pub price: i64,                 // Fixed-point nanodollars (divide by 1e9 for USD)
    pub size: u32,                  // Shares/contracts
    pub timestamp: Option<i64>,     // Nanoseconds since epoch (optional)
}
```

Key methods:
- `is_system_message() -> bool`: Returns true if `order_id == 0 || size == 0 || price <= 0`. These are heartbeats/status updates (~10-15% of real data).
- `price_as_f64() -> f64`: Converts price to dollars.
- `validate() -> Result<()>`: Checks order_id != 0, price > 0, size > 0.
- `with_timestamp(i64) -> Self`: Builder for setting timestamp.

### Action (`src/types.rs`)

```
#[repr(u8)]
pub enum Action {
    Add    = b'A',   // New order
    Modify = b'M',   // Modify existing
    Cancel = b'C',   // Cancel/remove
    Trade  = b'T',   // Trade execution
    Fill   = b'F',   // Fill (alternative trade)
    Clear  = b'R',   // Reset book
    None   = b'N',   // No-op
}
```

Methods: `from_byte(u8) -> Option<Self>`, `to_byte() -> u8`.

### Side (`src/types.rs`)

```
#[repr(u8)]
pub enum Side {
    Bid  = b'B',   // Buy order
    Ask  = b'A',   // Sell order
    None = b'N',   // Non-directional
}
```

Methods: `from_byte(u8) -> Option<Self>`, `to_byte() -> u8`, `is_bid() -> bool`, `is_ask() -> bool`.

### LobState (`src/types.rs`)

~560 bytes, stack-allocated. All arrays are `[_; MAX_LOB_LEVELS]` where `MAX_LOB_LEVELS = 20`.

```
pub struct LobState {
    // Book data (index 0 = best level)
    pub bid_prices: [i64; 20],           // Nanodollars, highest to lowest
    pub bid_sizes:  [u32; 20],           // Shares
    pub ask_prices: [i64; 20],           // Nanodollars, lowest to highest
    pub ask_sizes:  [u32; 20],           // Shares
    pub best_bid: Option<i64>,           // Cached best bid price
    pub best_ask: Option<i64>,           // Cached best ask price
    pub levels: usize,                   // Active levels (<= 20)
    pub timestamp: Option<i64>,          // Snapshot timestamp (ns since epoch)
    pub sequence: u64,                   // Message sequence number

    // Temporal fields (FI-2010 features u6-u9)
    pub previous_timestamp: Option<i64>, // Previous snapshot timestamp
    pub delta_ns: u64,                   // Time since last update (ns)
    pub triggering_action: Option<Action>,
    pub triggering_side: Option<Side>,
}
```

Analytics methods on `LobState`:
- `mid_price() -> Option<f64>` -- (best_bid + best_ask) / 2 in USD
- `spread() -> Option<f64>` -- best_ask - best_bid in USD
- `spread_bps() -> Option<f64>` -- spread / mid_price * 10000
- `microprice() -> Option<f64>` -- volume-weighted mid-price
- `total_bid_volume() -> u64`, `total_ask_volume() -> u64`
- `depth_imbalance() -> Option<f64>` -- (bid_vol - ask_vol) / (bid_vol + ask_vol)
- `check_consistency() -> BookConsistency`
- `delta_seconds() -> Option<f64>`, `event_intensity() -> Option<f64>`
- `was_triggered_by(Action) -> bool`, `was_triggered_on_bid() -> bool`, `is_trade_event() -> bool`

### BookConsistency (`src/types.rs`)

```
pub enum BookConsistency { Valid, Empty, Locked, Crossed }
```

Methods: `is_valid()`, `is_crossed()`, `is_locked()`, `is_empty()`.

### Order (`src/types.rs`)

```
pub struct Order { pub side: Side, pub price: i64, pub size: u32 }
```

### Config Structs

**LobConfig** (`src/lob/reconstructor.rs`):

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `levels` | `usize` | 10 | Price levels to track |
| `crossed_quote_policy` | `CrossedQuotePolicy` | `Allow` | How to handle crossed quotes |
| `validate_messages` | `bool` | `true` | Validate messages before processing |
| `log_warnings` | `bool` | `true` | Log warnings for consistency issues |
| `skip_system_messages` | `bool` | `true` | Skip order_id=0, size=0, price<=0 |

Builder: `new(levels)`, `with_crossed_quote_policy()`, `with_validation()`, `with_logging()`, `with_skip_system_messages()`.

**QueuePositionConfig** (`src/lob/queue_position.rs`):

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `track_position_changes` | `bool` | `false` | Enable PositionChange events |
| `max_position_changes` | `usize` | 1000 | Max retained changes |

Builder: `with_change_tracking()`, `research()` preset (10,000 changes).

**OrderLifecycleConfig** (`src/lob/order_lifecycle.rs`):

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `max_completed_retention` | `usize` | 10,000 | Max completed lifecycles kept |
| `track_modifications` | `bool` | `true` | Track modification history |
| `infer_pre_existing` | `bool` | `true` | Create synthetic lifecycles for unknown orders |
| `max_modifications_per_order` | `usize` | 100 | Max modifications stored per order |

Presets: `minimal()` (0 retention, no tracking), `research()` (100K retention, 1000 mods).

**TradeAggregatorConfig** (`src/lob/trade_aggregator.rs`):

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `max_recent_trades` | `usize` | 1000 | Rolling trade buffer size |
| `aggregation_window_ns` | `i64` | 1,000,000 (1ms) | Time window to merge fills |
| `track_fills` | `bool` | `false` | Store individual fills in Trade |

Builder: `with_max_trades()`, `with_aggregation_window_ns()`, `with_fill_tracking()`.

**DayBoundaryConfig** (`src/lob/day_boundary.rs`):

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `market_open_ns` | `i64` | 14h30m (9:30 AM ET in UTC) | Open time as ns from midnight UTC |
| `market_close_ns` | `i64` | 21h (4:00 PM ET in UTC) | Close time as ns from midnight UTC |
| `gap_threshold_ns` | `i64` | 4 * NS_PER_HOUR | Minimum gap for day boundary |
| `timezone_offset_hours` | `i32` | -5 (EST) | UTC offset |
| `use_gap_detection` | `bool` | `true` | Gap-based vs fixed-boundary detection |

Presets: `us_equity()` (default), `us_futures()`, `crypto()`.

**WarningTrackerConfig** (`src/warnings.rs`):

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `max_warnings` | `usize` | 100,000 | Max warnings retained in memory |
| `log_to_stderr` | `bool` | `true` | Print warnings to stderr |
| `min_log_severity` | `u8` | 1 | Minimum severity to log (1=all) |
| `deduplicate` | `bool` | `true` | Deduplicate similar warnings |
| `dedupe_window_ns` | `u64` | NS_PER_SECOND (1s) | Dedup time window |

**HotStoreConfig** (`src/hotstore.rs`) [databento]:

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `hot_store_dir` | `PathBuf` | `"./hot_store"` | Directory for decompressed files |
| `prefer_decompressed` | `bool` | `true` | Use decompressed file when available |
| `compressed_ext` | `String` | `".zst"` | Compressed file extension |
| `decompressed_ext` | `String` | `""` | Decompressed file extension |

Preset: `dbn_defaults(dir)` sets `.dbn.zst` -> `.dbn`.

**ExportConfig** (`src/export/mod.rs`) [export]:

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `levels` | `usize` | 10 | LOB levels to export |
| `include_derived` | `bool` | `true` | Include analytics columns |
| `include_mbo_events` | `bool` | `true` | Also export MBO events |
| `batch_size` | `usize` | 65,536 | Rows per Parquet row group |
| `compression` | `Compression` | SNAPPY | Parquet codec |
| `downsample` | `Option<DownsampleConfig>` | `None` | Optional downsampling |

### Stats Structs

**LobStats** (`src/lob/reconstructor.rs`) -- 17 fields:

```
pub struct LobStats {
    pub messages_processed: u64,
    pub system_messages_skipped: u64,
    pub active_orders: usize,
    pub bid_levels: usize,
    pub ask_levels: usize,
    pub errors: u64,
    pub crossed_quotes: u64,
    pub locked_quotes: u64,
    pub last_timestamp: Option<i64>,
    // Warning counters (6 fields):
    pub cancel_order_not_found: u64,
    pub cancel_price_level_missing: u64,
    pub cancel_order_at_level_missing: u64,
    pub trade_order_not_found: u64,
    pub trade_price_level_missing: u64,
    pub trade_order_at_level_missing: u64,
    // Additional:
    pub book_clears: u64,
    pub noop_messages: u64,
}
```

**QueueStats** (`src/lob/queue_position.rs`):

```
pub struct QueueStats {
    pub orders_tracked: u64,
    pub orders_removed: u64,
    pub position_queries: u64,
    pub position_changes_recorded: u64,
    pub messages_skipped: u64,
    pub cancel_not_found: u64,
    pub fill_not_found: u64,
}
```

**LifecycleStats** (`src/lob/order_lifecycle.rs`):

```
pub struct LifecycleStats {
    pub observed_orders: u64,
    pub inferred_orders: u64,
    pub total_modifications: u64,
    pub total_fills: u64,
    pub cancelled_orders: u64,
    pub filled_orders: u64,
    pub partial_then_cancelled: u64,
    pub active_orders: u64,
    pub completed_retained: u64,
    pub completed_evicted: u64,
    pub modifications_dropped: u64,
    pub messages_skipped: u64,
    pub overfill_count: u64,
}
```

**DayBoundaryStats** (`src/lob/day_boundary.rs`):

```
pub struct DayBoundaryStats {
    pub messages: u64,
    pub trades: u64,
    pub first_ts: Option<i64>,
    pub last_ts: Option<i64>,
    pub total_volume: u64,
    pub snapshots: u64,
}
```

### Event Types

**LifecycleEvent** (`src/lob/order_lifecycle.rs`):

```
pub enum LifecycleEvent {
    Created(OrderLifecycle),
    Modified { order_id: u64, modification: OrderModification },
    PartialFill { order_id: u64, fill_size: u32, fill_price: i64, timestamp: i64 },
    Completed(OrderLifecycle),
}
```

**PositionChange** (`src/lob/queue_position.rs`):

```
pub struct PositionChange {
    pub order_id: u64,
    pub timestamp: i64,
    pub old_position: Option<usize>,
    pub new_position: Option<usize>,
    pub volume_ahead_change: i64,    // Currently always 0 (reserved)
    pub reason: PositionChangeReason,
}
```

**PositionChangeReason** (`src/lob/queue_position.rs`):

```
pub enum PositionChangeReason {
    Added,              // Order joined queue
    Removed,            // Order left queue (cancel/fill)
    ImprovedByRemoval,  // Reserved: not yet emitted
    ModifiedPrice,      // Price change moves to back of queue
}
```

**Trade** (`src/lob/trade_aggregator.rs`):

```
pub struct Trade {
    pub timestamp: i64,
    pub price: i64,           // Nanodollars
    pub size: u64,            // Total aggregated size
    pub aggressor_side: Side, // Who initiated (Bid=buyer aggressed, Ask=seller aggressed)
    pub num_fills: u32,
    pub tick_direction: i8,   // +1 uptick, -1 downtick, 0 same
    pub fills: Option<Vec<Fill>>,  // Only if track_fills enabled
}
```

**Fill** (`src/lob/trade_aggregator.rs`):

```
pub struct Fill {
    pub timestamp: i64,
    pub price: i64,
    pub size: u32,
    pub order_id: u64,
}
```

**DayBoundary** (`src/lob/day_boundary.rs`):

```
pub struct DayBoundary {
    pub previous_day_end_ts: i64,
    pub new_day_start_ts: i64,
    pub gap_ns: i64,
    pub previous_day_index: u32,
    pub new_day_index: u32,
    pub previous_day_stats: DayBoundaryStats,
}
```

---

## 6. Core Design Patterns

### Unified Reduction Pattern

**File**: `src/lob/reconstructor.rs`, `reduce_or_remove_order()` (line ~544).

Cancel and Trade operations follow the same 4-stage lookup with identical partial/full removal logic. An `OrderReductionOp` enum (Cancel | Trade) selects which stat counters to increment:

1. **Order lookup** in `self.orders` -- not found: increment `{cancel,trade}_order_not_found`, return Ok.
2. **Price level lookup** in bids/asks -- not found: cleanup orphan order, increment `{cancel,trade}_price_level_missing`, return Ok.
3. **Order-at-level lookup** -- not found: cleanup orphan, increment `{cancel,trade}_order_at_level_missing`, return Ok.
4. **Size reduction**: If `msg.size >= current_size`, full removal. Otherwise partial reduction via `PriceLevel::reduce_order()`.

The same pattern exists in `QueuePositionTracker` (`src/lob/queue_position.rs`) with `ReductionOp` (Cancel | Fill) and `handle_order_reduction()`. This eliminated ~90% code duplication that was the root cause of audit bugs #2 and #3.

### Cached Invariant Pattern

**`PriceLevel`** (`src/lob/price_level.rs`): `total_size: u32` is maintained by `add_order()`, `remove_order()`, `reduce_order()`, and `update_order_size()` -- all encapsulated methods that update the cache atomically. `verify_invariant()` runs in debug builds (`#[cfg(debug_assertions)]`) via `debug_assert_eq!(compute_actual_total(), total_size)`.

**`QueueLevel`** (`src/lob/queue_position.rs`, private): `total_volume: u64` follows the same mutation-encapsulation pattern with `add_order()`, `remove_order()`, `reduce_order()`, `update_order_size()`. Unlike PriceLevel, QueueLevel does not have `verify_invariant()`/`debug_assert` in debug builds.

### Zero-Allocation Hot Path

`process_message_into(&mut self, msg: &MboMessage, state: &mut LobState) -> Result<()>` fills a pre-allocated `LobState` buffer (~560 bytes, stack-allocated). This is the single source of truth for dispatch, stats, and policy logic. `process_message()` delegates to it with a fresh buffer.

### Temporal Field Chain

When using `process_message_into()` with a reused `LobState` buffer, `fill_lob_state_with_temporal()` preserves temporal continuity:
- Reads `state.timestamp` as `previous_ts` BEFORE clearing the buffer.
- Computes `delta_ns = current_ts - previous_ts` (or 0 if timestamps unavailable).
- Sets `state.previous_timestamp = previous_ts`.

When using `process_message()` (fresh buffer each call), `delta_ns` is always 0 and `previous_timestamp` is always None -- no temporal chain exists.

### Composable Standalone Trackers

All four trackers (`QueuePositionTracker`, `OrderLifecycleTracker`, `TradeAggregator`, `DayBoundaryDetector`) have `process_message(&mut self, msg: &MboMessage)` methods that operate independently. Each filters system messages internally, maintains its own stats, and requires no access to `LobReconstructor`.

### System Message Filtering

`MboMessage::is_system_message()` (`src/types.rs`): returns true if `order_id == 0 || size == 0 || price <= 0`. Called by `LobReconstructor` (when `skip_system_messages` is true), all four trackers, and exposed for external filtering.

### QueueLevel vs PriceLevel Zero-Size Handling

`QueueLevel::reduce_order()` auto-removes orders that reach size 0 -- zero-size entries corrupt FIFO position calculations (queue_length, position_percentile, volume_ahead). `PriceLevel::reduce_order()` does NOT auto-remove zero-size orders -- lifecycle tracking is managed separately by the reconstructor's `reduce_or_remove_order()`.

### Inferred Lifecycle

`OrderLifecycleTracker` creates synthetic `OrderLifecycle` with `origin = Inferred` when it sees Modify/Cancel/Fill for an unknown order. This handles MBO streams that start mid-session where orders already exist. Inferred orders are tracked separately in `LifecycleStats::inferred_orders` and can be filtered by `OrderLifecycle::is_observed()`.

---

## 7. Internal Data Structures

| Structure | Backing | Location | Purpose |
|-----------|---------|----------|---------|
| Bid/Ask price levels | `BTreeMap<i64, PriceLevel>` | `reconstructor.rs` | Sorted price levels; bids iterated in reverse (highest first), asks forward (lowest first) |
| Order index | `AHashMap<u64, Order>` | `reconstructor.rs` | O(1) order lookup for modify/cancel/trade |
| Orders at price | `AHashMap<u64, u32>` | `price_level.rs` | order_id -> size within a PriceLevel |
| Queue levels (bid/ask) | `AHashMap<i64, QueueLevel>` | `queue_position.rs` | price -> FIFO-ordered queue |
| FIFO queue | `IndexMap<u64, u32>` | `queue_position.rs` (QueueLevel) | Insertion-ordered order_id -> size with O(1) lookup via `get_index_of()` |
| Position changes | `VecDeque<PositionChange>` | `queue_position.rs` | Capped history buffer |
| Active lifecycles | `AHashMap<u64, OrderLifecycle>` | `order_lifecycle.rs` | order_id -> lifecycle |
| Completed lifecycles | `VecDeque<OrderLifecycle>` | `order_lifecycle.rs` | Capped FIFO retention |
| Recent trades | `VecDeque<Trade>` | `trade_aggregator.rs` | Capped rolling buffer |
| Multi-symbol LOBs | `AHashMap<String, LobReconstructor>` | `multi_symbol.rs` | symbol -> reconstructor |

---

## 8. Config Validation

All 8 config structs implement `validate() -> Result<()>` which returns `Err(TlobError::InvalidConfig(...))` on degenerate values. The pattern for constructors:

- **Panicking** (`-> Self`): Uses `.expect()` in constructors like `LobReconstructor::with_config()`, `QueuePositionTracker::new()`, `OrderLifecycleTracker::new()`, `TradeAggregator::new()`, `DayBoundaryDetector::new()`, `WarningTracker::with_config()`. These panic at construction time if config is invalid.
- **Fallible** (`-> Result<Self>`): `LobSnapshotWriter::new()` and `MboEventWriter::new()` call `config.validate()?`.

Validation rules include: levels in [1, 20], max_warnings > 0, batch_size > 0, aggregation_window_ns >= 0, max_recent_trades > 0, gap_threshold_ns >= 0, etc.

---

## 9. Error Handling

**`TlobError`** (`src/error.rs`) -- 12 variants:

| Variant | Payload | Category |
|---------|---------|----------|
| `InvalidOrderId(u64)` | order_id | Hard (validation) |
| `OrderNotFound(u64)` | order_id | Hard |
| `InvalidPrice(i64)` | price | Hard (validation) |
| `InvalidSize(u32)` | size | Hard (validation) |
| `InvalidAction(u8)` | byte | Hard (decode) |
| `InvalidSide(u8)` | byte | Hard (decode) |
| `SymbolNotFound(String)` | symbol | Hard (multi-symbol) |
| `InconsistentState(String)` | description | Hard |
| `CrossedQuote(i64, i64)` | best_bid, best_ask | Hard (only with `CrossedQuotePolicy::Error`) |
| `LockedQuote(i64, i64)` | best_bid, best_ask | Hard (validation path) |
| `InvalidConfig(String)` | description | Hard (startup) |
| `Generic(String)` | message | Hard (I/O, misc) |

**Soft errors** (tracked in stats, never return Err):
- Cancel/Trade for unknown order -> `{cancel,trade}_order_not_found` stat
- Cancel/Trade with missing price level -> `{cancel,trade}_price_level_missing` stat
- Cancel/Trade with order not at expected level -> `{cancel,trade}_order_at_level_missing` stat
- Crossed/locked quotes (with `Allow`, `UseLastValid`, or `SkipUpdate` policy) -> `crossed_quotes`/`locked_quotes` stats

**Hard errors** (return Err):
- Message validation failure (when `validate_messages = true`)
- `CrossedQuotePolicy::Error` when book crosses
- Config validation failure
- I/O errors (file not found, decode failure)

`From` implementations: `std::io::Error`, `String`, `&str` all convert to `TlobError::Generic`.

---

## 10. Reset Semantics

### LobReconstructor

| Method | Clears Book | Clears Stats | Use Case |
|--------|-------------|--------------|----------|
| `reset()` | Yes (bids, asks, orders, best_bid, best_ask, last_valid_state) | No | Mid-session `Action::Clear` received |
| `full_reset()` | Yes (calls `reset()`) | Yes (stats = default) | New trading day or symbol |

`reset()` preserves stats so cumulative metrics can be tracked across mid-session clears. `full_reset()` resets everything for a fresh start.

### Trackers

Each tracker should be reconstructed or have its own reset logic applied between trading days. They are independent stateful objects.

---

## 11. Crossed Quote Policies

**`CrossedQuotePolicy`** (`src/lob/reconstructor.rs`):

| Policy | Returned LobState | Internal Book Mutation | Stats Tracking |
|--------|-------------------|----------------------|----------------|
| `Allow` (default) | Current state (crossed) | Yes | `crossed_quotes += 1` |
| `UseLastValid` | Last valid snapshot | Yes (order IS processed internally) | `crossed_quotes += 1`, temporal fields patched to current message |
| `SkipUpdate` | Last valid snapshot | Yes (order IS processed internally) | `crossed_quotes += 1`, temporal fields patched to current message |
| `Error` | `Err(TlobError::CrossedQuote)` | Yes (order IS processed before error check) | `crossed_quotes += 1` |

**Critical**: `SkipUpdate` and `UseLastValid` are behaviorally identical -- both allow the internal book to mutate (the order is added/cancelled/traded) and both return the last valid LobState snapshot. They share the same code path (`CrossedQuotePolicy::UseLastValid | CrossedQuotePolicy::SkipUpdate => { ... }`). The name "SkipUpdate" is misleading; it does NOT skip the internal book mutation.

When `UseLastValid` or `SkipUpdate` returns a last-valid snapshot, temporal fields are patched to maintain the caller's temporal chain: `triggering_action`, `triggering_side`, `timestamp`, `previous_timestamp`, and `delta_ns` come from the current message, while book data (prices, sizes, best_bid, best_ask) come from the last valid state.

---

## 12. Constants

**`src/constants.rs`** -- 10 named constants:

| Constant | Type | Value | Purpose |
|----------|------|-------|---------|
| `NANODOLLARS_PER_DOLLAR` | `i64` | `1_000_000_000` | Integer price conversion |
| `NANODOLLARS_PER_DOLLAR_F64` | `f64` | `1_000_000_000.0` | Float price conversion (exact in IEEE 754) |
| `BASIS_POINTS_PER_UNIT` | `f64` | `10_000.0` | Ratio to bps conversion |
| `DIVISION_GUARD_EPS` | `f64` | `1e-8` | Near-zero denominator guard (~sqrt(f64::EPSILON)) |
| `NS_PER_MILLISECOND` | `i64` | `1_000_000` | Time conversion |
| `NS_PER_SECOND` | `i64` | `1_000_000_000` | Time conversion |
| `NS_PER_SECOND_F64` | `f64` | `1_000_000_000.0` | Float time conversion (exact in IEEE 754) |
| `NS_PER_MINUTE` | `i64` | `60_000_000_000` | Time conversion |
| `NS_PER_HOUR` | `i64` | `3_600_000_000_000` | Time conversion |
| `NS_PER_DAY` | `i64` | `86_400_000_000_000` | Time conversion |

Additionally, from `src/loader.rs`: `IO_BUFFER_SIZE: usize = 1_048_576` (1 MB).

From `src/types.rs`: `MAX_LOB_LEVELS: usize = 20`.

From `src/export/mod.rs`: `SCHEMA_VERSION: &str = "1.0"`, `DEFAULT_BATCH_SIZE: usize = 65_536`.

---

## 13. Export System

### Schema Version

`SCHEMA_VERSION = "1.0"` (embedded in every Parquet file's metadata). Any breaking schema change requires a version bump.

### LOB Snapshot Schema (`lob_snapshot_schema()`)

12 core columns + 8 optional derived columns:

**Core** (always present):

| Column | Arrow Type | Nullable |
|--------|-----------|----------|
| `timestamp_ns` | Int64 | false |
| `sequence` | UInt64 | false |
| `levels` | UInt8 | false |
| `best_bid` | Int64 | true |
| `best_ask` | Int64 | true |
| `bid_prices` | FixedSizeList(Int64, N) | false |
| `bid_sizes` | FixedSizeList(UInt32, N) | false |
| `ask_prices` | FixedSizeList(Int64, N) | false |
| `ask_sizes` | FixedSizeList(UInt32, N) | false |
| `delta_ns` | UInt64 | false |
| `triggering_action` | UInt8 | true |
| `triggering_side` | UInt8 | true |

**Derived** (when `include_derived = true`):

| Column | Arrow Type | Nullable |
|--------|-----------|----------|
| `mid_price` | Float64 | true |
| `spread` | Float64 | true |
| `spread_bps` | Float64 | true |
| `microprice` | Float64 | true |
| `total_bid_volume` | UInt64 | false |
| `total_ask_volume` | UInt64 | false |
| `depth_imbalance` | Float64 | true |
| `book_consistency` | UInt8 | false |

`book_consistency` encoding: 0=Valid, 1=Empty, 2=Locked, 3=Crossed.

### MBO Event Schema (`mbo_event_schema()`)

6 columns:

| Column | Arrow Type | Nullable |
|--------|-----------|----------|
| `timestamp_ns` | Int64 | true |
| `order_id` | UInt64 | false |
| `action` | UInt8 | false |
| `side` | UInt8 | false |
| `price` | Int64 | false |
| `size` | UInt32 | false |

### File Metadata

Every Parquet file includes: `schema_version`, `source` ("mbo-lob-reconstructor"), `reconstructor_version`, `price_unit` ("nanodollars"), `size_unit` ("shares"), `timestamp_unit` ("nanoseconds_since_epoch"). `LobSnapshotWriter` additionally embeds `lob_levels` and any user-provided `extra_metadata`.

### Writers

`LobSnapshotWriter::new(path, &ExportConfig, extra_metadata)` -- buffers `LobState` snapshots in column-oriented `LobBatch`, flushes to Parquet row groups at `batch_size`. Supports downsampling via `DownsampleStrategy::{None, EveryN(usize), MinIntervalNs(u64)}`.

`MboEventWriter::new(path, &ExportConfig, extra_metadata)` -- buffers `MboMessage` events in `MboBatch`, same flush logic.

Both return `ParquetExportStats` from `finish()`.

---

## 14. CLI Binaries

### `export_to_parquet`

Required features: `databento`, `export`.

```bash
cargo run --release --features export --bin export_to_parquet -- \
    --config configs/nvda_full_export.toml

cargo run --release --features export --bin export_to_parquet -- \
    --input data/NVDA.mbo.dbn.zst \
    --output data/exports/raw_lob/ \
    --symbol NVDA \
    --levels 10 \
    --compression snappy
```

Supports TOML config files with `[export]`, `[input]`, `[output]` sections. CLI flags override TOML values. Options include `--levels`, `--include-derived`, `--include-mbo`, `--batch-size`, `--compression`, `--downsample-every`, `--downsample-interval-ns`, `--hot-store`, `--verbose`.

### `decompress_to_hot_store`

Required features: `databento`.

```bash
cargo run --release --bin decompress_to_hot_store -- \
    --input data/XNAS_ITCH/NVDA/mbo_2025-02-03_to_2026-01-07/ \
    --output data/hot/ \
    --dry-run

cargo run --release --bin decompress_to_hot_store -- \
    -i data/NVDA.mbo.dbn.zst \
    -o data/hot/ \
    --force --verbose
```

Flags: `--input` / `-i` (file or directory), `--output` / `-o` (hot store dir), `--dry-run` / `-n`, `--force` / `-f` (re-decompress existing), `--verbose` / `-v`.

---

## 15. Module Dependency Graph

```
                        lib.rs
                          |
          +-------+-------+-------+--------+--------+
          |       |       |       |        |        |
       types.rs  error.rs constants.rs  source.rs  warnings.rs
          |                                |
          |                         [databento]
          |                      /      |       \
          |               loader.rs  hotstore.rs  dbn_bridge.rs
          |                   \        /
          |                    DbnSource
          |
    +-----+------+
    |     lob/    |
    |             |
    |   mod.rs ---+-----+----------+----------+-----------+
    |        |          |          |          |           |
    |  reconstructor.rs |   queue_position.rs |   day_boundary.rs
    |     |             |          |          |
    |  price_level.rs   |   order_lifecycle.rs|
    |                   |                     |
    |            multi_symbol.rs    trade_aggregator.rs
    |
    +--- statistics.rs  (depends on: types, constants)
    |
    +--- analytics.rs   (depends on: types, constants)
    |
    +--- [export]
         |
         export/mod.rs ---+--- schema.rs (depends on: types)
                          +--- lob_writer.rs (depends on: types, export/batch, export/schema)
                          +--- mbo_writer.rs (depends on: types, export/batch, export/schema)
                          +--- batch.rs (depends on: types, export/schema)

    CLI binaries:
    bin/export_to_parquet.rs      [databento, export]
    bin/decompress_to_hot_store.rs [databento]
```

Feature gates:
- `[databento]`: `loader.rs`, `hotstore.rs`, `dbn_bridge.rs`, `DbnSource` in `source.rs`
- `[export]`: entire `export/` module

All modules under `lob/` depend on subsets of `types.rs`, `constants.rs`, and `error.rs`. The four standalone trackers (`queue_position`, `order_lifecycle`, `trade_aggregator`, `day_boundary`) do NOT depend on `reconstructor.rs` or `price_level.rs` -- they are fully independent.
