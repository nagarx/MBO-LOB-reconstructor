# MBO-LOB-Reconstructor: Codebase Technical Reference

> **Purpose**: This document provides complete technical details for LLMs and developers to understand, modify, and extend the codebase without prior context.

> **Pipeline scope (2026-06-02).** This module is part of an **intraday trading research pipeline** — an experiment-first platform for discovering and validating *any* profitable **intraday** trading edge (no overnight positions), across approach classes (microstructure/HFT, scalping, intraday momentum, intraday statistical arbitrage, …) and instruments (equities, futures, same-day options). The pipeline *originated* as a high-frequency NVDA MBO/LOB microstructure system — that origin explains the "HFT" / "LOB" / "MBO" naming here — and that microstructure-direction program is now one (largely-closed) track among many. **Names are historical; the mission is general.** This module's role: the Rust ingestion front-end — reconstructs limit-order-book state (`LobState`) from raw Market-By-Order `.dbn.zst` events (~1M msg/s, BBO 99.17%); the order-book source feeding feature extraction. For the full mission + approach taxonomy + capability-readiness boundary, see root `CLAUDE.md` §Research Scope & Charter (+ `CROSS_ASSET_OFI_FINDINGS_AND_ISSUES_2026_06_01.md` §9).

---

## Table of Contents

1. [Project Overview](#1-project-overview)
2. [Module Architecture](#2-module-architecture)
3. [Core Types and Data Structures](#3-core-types-and-data-structures)
4. [Processing Pipeline](#4-processing-pipeline)
5. [Key Algorithms](#5-key-algorithms)
6. [Configuration Options](#6-configuration-options)
7. [Error Handling](#7-error-handling)
8. [Statistics and Analytics](#8-statistics-and-analytics)
9. [Testing Patterns](#9-testing-patterns)
10. [Performance Considerations](#10-performance-considerations)
11. [Common Patterns and Idioms](#11-common-patterns-and-idioms)
12. [Integration with Feature Extractor](#12-integration-with-feature-extractor)
13. [Known Limitations and Edge Cases](#13-known-limitations-and-edge-cases)
14. [Composable Tracking Modules](#14-composable-tracking-modules)
15. [Parquet Export Module](#15-parquet-export-module-feature-export)

---

## 1. Project Overview

### What This Library Does

Converts Market-By-Order (MBO) data streams into Limit Order Book (LOB) snapshots. MBO data contains individual order events (add, modify, cancel, trade), and this library reconstructs the aggregated price-level view.

### Key Capabilities

| Capability | Description |
|------------|-------------|
| LOB Reconstruction | MBO events → price-level aggregation |
| System Message Filtering | Auto-skip heartbeats, status updates (order_id=0) |
| Crossed Quote Handling | Configurable policies for bid ≥ ask |
| Temporal Fields | Time delta, triggering action/side (FI-2010 u6-u9) |
| Analytics | Microprice, VWAP, depth imbalance, market impact |
| Statistics | Welford's algorithm for streaming mean/std |
| Multi-Symbol | Manage multiple LOBs simultaneously |
| Queue Position Tracking | FIFO position, volume ahead (composable module) |
| Order Lifecycle Tracking | Add→Modify→Cancel/Fill lifecycle (composable module) |
| Day Boundary Detection | Trading day boundaries for train/test splits |
| Trade Aggregation | Fills→trades with aggressor side detection |
| DBN Support | Native Databento file loading (feature-gated) |

### Directory Structure

```
src/
├── lib.rs              # Public API, re-exports
├── types.rs            # MboMessage, LobState (with temporal fields), Action, Side, MAX_LOB_LEVELS
├── error.rs            # TlobError, Result type
├── lob/
│   ├── mod.rs          # Module overview
│   ├── reconstructor.rs # LobReconstructor core logic (with temporal population)
│   ├── price_level.rs  # PriceLevel with cached total_size (O(1) queries)
│   ├── multi_symbol.rs # MultiSymbolLob manager
│   ├── day_boundary.rs # DayBoundaryDetector, DayBoundaryConfig
│   ├── trade_aggregator.rs # TradeAggregator, Trade, Fill
│   ├── order_lifecycle.rs # OrderLifecycleTracker, OrderLifecycle
│   └── queue_position.rs # QueuePositionTracker (FIFO with IndexMap)
├── export/             # Parquet export (requires `export` feature)
│   ├── mod.rs          # ExportConfig, DownsampleConfig, DownsampleStrategy
│   ├── schema.rs       # Arrow schemas for LOB snapshots & MBO events
│   ├── lob_writer.rs   # LobSnapshotWriter: LobState → Parquet rows
│   ├── mbo_writer.rs   # MboEventWriter: MboMessage → Parquet rows
│   └── batch.rs        # Column-oriented batching (LobBatch, MboBatch)
├── source.rs           # MarketDataSource trait, DbnSource, VecSource
├── hotstore.rs         # HotStoreConfig, HotStoreManager
├── loader/
│   ├── mod.rs          # DbnLoader for file I/O (auto-detects compression); TypedMessageIterator (preferred) + legacy MessageIterator
│   └── error.rs        # BoundaryError — typed error domain for the loader yield path
├── dbn_bridge.rs       # Databento format conversion
├── constants.rs        # Domain constants: NANODOLLARS_PER_DOLLAR, NS_PER_SECOND, BASIS_POINTS_PER_UNIT, DIVISION_GUARD_EPS (10 total)
├── statistics.rs       # RunningStats, DayStats, NormalizationParams
├── analytics.rs        # DepthStats, MarketImpact, LiquidityMetrics
├── warnings.rs         # WarningTracker, WarningCategory
└── bin/
    ├── decompress_to_hot_store.rs  # CLI tool for hot store population
    └── export_to_parquet.rs        # CLI tool for DBN → Parquet export
```

---

## 2. Module Architecture

### Module Dependency Graph

```
┌─────────────────────────────────────────────────────────────────┐
│                         lib.rs (public API)                      │
└─────────────────────────────────────────────────────────────────┘
         │              │              │              │
         ▼              ▼              ▼              ▼
┌─────────────┐  ┌────────────┐  ┌───────────┐  ┌───────────┐
│   lob/      │  │ statistics │  │ analytics │  │ warnings  │
│ (core LOB)  │  │ (ML stats) │  │ (metrics) │  │ (tracking)│
└─────────────┘  └────────────┘  └───────────┘  └───────────┘
         │              │              │
         ▼              ▼              ▼
┌─────────────────────────────────────────────────────────────────┐
│                types.rs + error.rs + constants.rs                 │
│        (MboMessage, LobState, TlobError, domain constants)       │
└─────────────────────────────────────────────────────────────────┘
         ▲
         │ (feature-gated: databento)
┌─────────────┐  ┌─────────────┐
│   loader    │──│ dbn_bridge  │
│ (file I/O)  │  │ (format)    │
└─────────────┘  └─────────────┘
```

### Module Responsibilities

| Module | Responsibility | Key Types |
|--------|---------------|-----------|
| `types` | Data structures, no logic | `MboMessage`, `LobState`, `Action`, `Side`, `Order`, `BookConsistency`, `MAX_LOB_LEVELS` |
| `constants` | Domain constants (10 total: prices, time, precision) | `NANODOLLARS_PER_DOLLAR`, `NANODOLLARS_PER_DOLLAR_F64`, `BASIS_POINTS_PER_UNIT`, `DIVISION_GUARD_EPS`, `NS_PER_MILLISECOND`, `NS_PER_SECOND`, `NS_PER_SECOND_F64`, `NS_PER_MINUTE`, `NS_PER_HOUR`, `NS_PER_DAY` |
| `error` | Error definitions (13 variants) | `TlobError`, `Result<T>` |
| `lob/reconstructor` | Core LOB reconstruction | `LobReconstructor`, `LobConfig`, `LobStats`, `CrossedQuotePolicy` |
| `lob/price_level` | Price level with cached size | `PriceLevel` (O(1) aggregate size) |
| `lob/multi_symbol` | Multi-stock management | `MultiSymbolLob` |
| `lob/queue_position` | FIFO queue position tracking | `QueuePositionTracker`, `QueuePositionConfig`, `QueuePositionInfo`, `QueueStats` |
| `lob/order_lifecycle` | Order lifecycle tracking | `OrderLifecycleTracker`, `OrderLifecycle`, `LifecycleEvent`, `LifecycleStats`, `ActiveOrderFeatures` |
| `lob/day_boundary` | Trading day detection | `DayBoundaryDetector`, `DayBoundaryConfig`, `DayBoundary`, `DayBoundaryStats` |
| `lob/trade_aggregator` | Trade aggregation | `TradeAggregator`, `Trade`, `Fill` |
| `source` | Provider abstraction | `MarketDataSource`, `SourceMetadata`, `DbnSource`, `VecSource` |
| `hotstore` | Decompressed data caching | `HotStoreConfig`, `HotStoreManager` |
| `loader` | DBN file streaming | `DbnLoader`, `TypedMessageIterator` (preferred: `iter_messages_typed()` → `Result<MboMessage, BoundaryError>`), `BoundaryError` (`loader/error.rs`), `LoaderStats`, `MessageIterator` (legacy — `#[deprecated]`, gated behind default-on `legacy-iterator-api`; removal 0.3.0 / 2026-10-29) |
| `dbn_bridge` | DBN → internal conversion | `DbnBridge` |
| `statistics` | ML statistics | `RunningStats`, `DayStats`, `NormalizationParams` |
| `analytics` | Market microstructure | `DepthStats`, `MarketImpact`, `LiquidityMetrics` |
| `warnings` | Issue tracking | `WarningTracker`, `WarningTrackerConfig`, `Warning`, `WarningCategory`, `WarningSummary` |
| `export` | Parquet export (feature-gated) | `ExportConfig`, `LobSnapshotWriter`, `MboEventWriter`, `ParquetExportStats` |

---

## 3. Core Types and Data Structures

### MboMessage (src/types.rs)

Input message representing a single order book event.

```rust
pub struct MboMessage {
    pub order_id: u64,        // Unique order identifier (0 = system message)
    pub action: Action,       // Add, Modify, Cancel, Trade, Fill, Clear, None
    pub side: Side,           // Bid, Ask, None
    pub price: i64,           // Fixed-point nanodollars (divide by NANODOLLARS_PER_DOLLAR_F64 for dollars)
    pub size: u32,            // Shares/contracts
    pub timestamp: Option<i64>, // Nanoseconds since epoch
}
```

**Critical**: Messages with `order_id=0`, `size=0`, or `price<=0` are **system messages** (heartbeats, status updates), not valid orders. Use `msg.is_system_message()` to check (replaces the deprecated `is_valid_order()` free function).

### Action Enum

```rust
pub enum Action {
    Add = b'A',      // New order
    Modify = b'M',   // Change existing order
    Cancel = b'C',   // Remove order (full or partial)
    Trade = b'T',    // Execution against order
    Fill = b'F',     // Alternative trade representation
    Clear = b'R',    // Reset entire book
    None = b'N',     // No-op (may carry metadata)
}
```

### Side Enum

```rust
pub enum Side {
    Bid = b'B',  // Buy order
    Ask = b'A',  // Sell order
    None = b'N', // Non-directional
}
```

### LobState (src/types.rs)

Output snapshot of the order book at N price levels. Uses fixed-size stack-allocated arrays.

```rust
pub struct LobState {
    // Core LOB data (stack-allocated, MAX_LOB_LEVELS = 20)
    pub bid_prices: [i64; MAX_LOB_LEVELS],  // Highest to lowest
    pub bid_sizes: [u32; MAX_LOB_LEVELS],   // Aggregated at each price
    pub ask_prices: [i64; MAX_LOB_LEVELS],  // Lowest to highest
    pub ask_sizes: [u32; MAX_LOB_LEVELS],   // Aggregated at each price
    pub best_bid: Option<i64>,              // Cached best prices
    pub best_ask: Option<i64>,
    pub levels: usize,                      // Number of levels tracked
    pub timestamp: Option<i64>,
    pub sequence: u64,                      // Message sequence number
    
    // Temporal fields (for time-sensitive features FI-2010 u6-u9)
    pub previous_timestamp: Option<i64>,    // For Δt calculation
    pub delta_ns: u64,                      // Time since last update
    pub triggering_action: Option<Action>,  // What caused this state
    pub triggering_side: Option<Side>,      // Which side was affected
}
```

**Temporal Helper Methods**:
- `delta_seconds()` - Time delta in seconds
- `event_intensity()` - Events per second (1/Δt)
- `was_triggered_by(action)` - Check triggering action
- `is_trade_event()`, `is_add_event()`, `is_cancel_event()` - Event type checks

### LobReconstructor Internal State (src/lob/reconstructor.rs)

```rust
pub struct LobReconstructor {
    config: LobConfig,
    bids: BTreeMap<i64, PriceLevel>,     // price → PriceLevel (with cached total_size)
    asks: BTreeMap<i64, PriceLevel>,     // price → PriceLevel (with cached total_size)
    orders: AHashMap<u64, Order>,        // order_id → Order (fast lookup)
    best_bid: Option<i64>,               // Cached best bid price
    best_ask: Option<i64>,               // Cached best ask price
    stats: LobStats,                     // Processing statistics
    last_valid_state: Option<LobState>,  // For UseLastValid crossed quote policy
}
```

### PriceLevel (src/lob/price_level.rs)

Each price level wraps a HashMap with a **cached aggregate size** for O(1) queries:

```rust
pub struct PriceLevel {
    orders: AHashMap<u64, u32>,  // order_id → size
    total_size: u32,             // Cached: always == orders.values().sum()
}

impl PriceLevel {
    pub fn add_order(&mut self, order_id: u64, size: u32) -> Option<u32>;
    pub fn remove_order(&mut self, order_id: u64) -> Option<u32>;
    pub fn reduce_order(&mut self, order_id: u64, delta: u32) -> Option<u32>;
    pub fn total_size(&self) -> u32;  // O(1) - uses cached value
    pub fn is_empty(&self) -> bool;
    pub fn order_count(&self) -> usize;
}
```

**Data Structure Rationale**:
- `BTreeMap<i64, PriceLevel>`: Keeps prices sorted (O(log n) insert, O(1) min/max)
- `PriceLevel`: Encapsulates order tracking with O(1) aggregate size (no more `values().sum()`)
- `AHashMap`: Fast hash map for order lookups (O(1) average)
- Cached `best_bid`/`best_ask`: Avoid BTreeMap traversal on every message

---

## 4. Processing Pipeline

### Main Processing Flow

```
┌─────────────┐     ┌─────────────────────────────────────────────────┐
│ MboMessage  │────▶│           LobReconstructor::process_message()   │
└─────────────┘     └─────────────────────────────────────────────────┘
                                        │
                    ┌───────────────────┼───────────────────┐
                    ▼                   ▼                   ▼
           ┌───────────────┐   ┌───────────────┐   ┌───────────────┐
           │ 1. Skip       │   │ 2. Validate   │   │ 3. Process    │
           │ System Msgs   │   │ Message       │   │ Action        │
           │ (if enabled)  │   │ (if enabled)  │   │               │
           └───────────────┘   └───────────────┘   └───────────────┘
                                                           │
                    ┌──────────────────────────────────────┼─────────┐
                    ▼              ▼              ▼        ▼         ▼
              ┌─────────┐   ┌──────────┐   ┌─────────┐ ┌───────┐ ┌───────┐
              │ Add     │   │ Modify   │   │ Cancel  │ │ Trade │ │ Clear │
              │ Order   │   │ Order    │   │ Order   │ │       │ │       │
              └─────────┘   └──────────┘   └─────────┘ └───────┘ └───────┘
                                                           │
                    ┌──────────────────────────────────────┘
                    ▼
           ┌───────────────┐
           │ 4. Update     │
           │ Statistics    │
           └───────────────┘
                    │
                    ▼
           ┌───────────────┐
           │ 5. Update     │
           │ Best Prices   │
           └───────────────┘
                    │
                    ▼
           ┌───────────────┐
           │ 6. Check      │
           │ Consistency   │
           └───────────────┘
                    │
                    ▼
           ┌───────────────┐
           │ 7. Apply      │
           │ Policy        │
           └───────────────┘
                    │
                    ▼
              ┌─────────────┐
              │  LobState   │
              └─────────────┘
```

### System Message Filtering (Step 1)

```rust
// In process_message()
if self.config.skip_system_messages
    && (msg.order_id == 0 || msg.size == 0 || msg.price <= 0)
{
    self.stats.system_messages_skipped += 1;
    return Ok(self.get_lob_state());  // Return current state unchanged
}
```

**Why this matters**: DBN data contains ~10-15% system messages. Without filtering, these would cause validation errors.

### Action Processing (Step 3)

| Action | Logic |
|--------|-------|
| **Add** | Insert order into price level, track in orders map |
| **Modify** | Remove old order, add new (handles price change) |
| **Cancel** | Reduce size or remove order; supports partial cancels |
| **Trade/Fill** | Reduce size or remove order (execution) |
| **Clear** | Call `reset()`, increment `book_clears` stat |
| **None** | No-op, increment `noop_messages` stat |

### Soft Error Handling in Cancel/Trade

Anomalies don't fail - they're tracked in stats:

```rust
// Cancel for unknown order
if order not found {
    self.stats.cancel_order_not_found += 1;
    return Ok(());  // Don't fail
}
```

This is intentional: market data often has late cancels, already-filled orders, etc.

---

## 5. Key Algorithms

### Order Book Price Aggregation

```
BTreeMap<price, PriceLevel>

Example bid side:
  $100.00 → PriceLevel { orders: {1001: 50, 1002: 100}, total_size: 150 }
  $99.99  → PriceLevel { orders: {1003: 200}, total_size: 200 }
```

The `PriceLevel` struct maintains a cached `total_size` that is updated on every mutation, enabling O(1) aggregate queries instead of O(n) sum operations.

### Best Price Update

```rust
fn update_best_prices(&mut self) {
    // BTreeMap iteration is sorted
    self.best_bid = self.bids.keys().next_back().copied();  // Highest
    self.best_ask = self.asks.keys().next().copied();       // Lowest
}
```

### Welford's Online Algorithm (src/statistics.rs)

For numerically stable streaming mean/std:

```rust
pub fn update(&mut self, value: f64) {
    self.count += 1;
    let delta = value - self.mean;
    self.mean += delta / self.count as f64;
    let delta2 = value - self.mean;
    self.m2 += delta * delta2;
}

pub fn std(&self) -> f64 {
    (self.m2 / self.count as f64).sqrt()
}
```

### Microprice Calculation (src/types.rs)

```rust
// Volume-weighted mid-price
microprice = (bid_price * ask_size + ask_price * bid_size) / (bid_size + ask_size)
```

When `ask_size > bid_size`, microprice is closer to bid (buying pressure).

---

## 6. Configuration Options

### LobConfig (src/lob/reconstructor.rs)

```rust
pub struct LobConfig {
    pub levels: usize,                    // Number of price levels (default: 10)
    pub crossed_quote_policy: CrossedQuotePolicy,  // How to handle bid ≥ ask
    pub validate_messages: bool,          // Run msg.validate() (default: true)
    pub log_warnings: bool,               // Log anomalies (default: true)
    pub skip_system_messages: bool,       // Skip order_id=0 etc. (default: true)
}
```

### CrossedQuotePolicy

```rust
pub enum CrossedQuotePolicy {
    Allow,        // Return crossed state as-is (default)
    UseLastValid, // Return last valid state (book IS mutated internally)
    Error,        // Return Err(CrossedQuote) or Err(LockedQuote)
    SkipUpdate,   // Same as UseLastValid (book IS mutated, returns last valid state)
}
// Note: SkipUpdate and UseLastValid share the same code path.
// Both allow internal book mutations; only the returned LobState is affected.
```

### Configuration Pattern

```rust
let config = LobConfig::new(10)
    .with_crossed_quote_policy(CrossedQuotePolicy::UseLastValid)
    .with_validation(true)
    .with_logging(false)
    .with_skip_system_messages(true);

let mut lob = LobReconstructor::with_config(config);
```

### QueuePositionConfig (src/lob/queue_position.rs)

```rust
pub struct QueuePositionConfig {
    pub track_position_changes: bool,    // Default: false (saves memory)
    pub max_position_changes: usize,     // Default: 1000
}

// Presets
QueuePositionConfig::default()      // Standard tracking
QueuePositionConfig::research()     // Full tracking (changes enabled, 10K history)
```

### OrderLifecycleConfig (src/lob/order_lifecycle.rs)

```rust
pub struct OrderLifecycleConfig {
    pub max_completed_retention: usize,     // Default: 10_000
    pub track_modifications: bool,          // Default: true
    pub infer_pre_existing: bool,           // Default: true (handle mid-session starts)
    pub max_modifications_per_order: usize, // Default: 100
}
```

### DayBoundaryConfig (src/lob/day_boundary.rs)

```rust
pub struct DayBoundaryConfig {
    pub market_open_ns: i64,        // Nanoseconds from midnight UTC
    pub market_close_ns: i64,       // Nanoseconds from midnight UTC
    pub gap_threshold_ns: i64,      // Default: 4 hours (overnight detection)
    pub timezone_offset_hours: i32, // Default: -5 (EST)
    pub use_gap_detection: bool,    // Default: true. If false, uses fixed midnight boundary
}

// Presets
DayBoundaryConfig::us_equity()  // 9:30 AM - 4:00 PM ET, gap detection
DayBoundaryConfig::us_futures() // Extended hours, gap detection
DayBoundaryConfig::crypto()     // 24/7, fixed midnight UTC boundary (use_gap_detection=false)
```

### TradeAggregatorConfig (src/lob/trade_aggregator.rs)

```rust
pub struct TradeAggregatorConfig {
    pub max_recent_trades: usize,      // Default: 1000
    pub aggregation_window_ns: i64,    // Default: 1_000_000 (1ms)
    pub track_fills: bool,             // Default: false
}
```

### WarningTrackerConfig (src/warnings.rs)

```rust
pub struct WarningTrackerConfig {
    pub max_warnings: usize,       // Default: 100_000 (cap on stored warnings)
    pub log_to_stderr: bool,       // Default: true
    pub min_log_severity: u8,      // Default: 1 (log all severities)
    pub deduplicate: bool,         // Default: true (hash-based dedup within time window)
    pub dedupe_window_ns: u64,     // Default: 1 second (NS_PER_SECOND)
}
```

### HotStoreConfig (src/hotstore.rs)

```rust
pub struct HotStoreConfig {
    pub hot_store_dir: PathBuf,       // Directory for decompressed files
    pub prefer_decompressed: bool,    // Default: true (use hot store if available)
    pub compressed_ext: String,       // Default: ".zst" (or ".dbn.zst" with dbn_defaults)
    pub decompressed_ext: String,     // Default: "" (or ".dbn" with dbn_defaults)
}

// Factory methods
HotStoreConfig::new(dir)           // Generic defaults (.zst → "")
HotStoreConfig::dbn_defaults(dir)  // DBN defaults (.dbn.zst → .dbn)
```

---

## 7. Error Handling

### TlobError Variants (src/error.rs)

```rust
pub enum TlobError {
    InvalidOrderId(u64),       // order_id == 0
    OrderNotFound(u64),        // Operation on missing order
    InvalidPrice(i64),         // price <= 0
    InvalidSize(u32),          // size == 0
    InvalidTimestamp(i64),     // ts_event <= 0 or u64->i64 overflow (M.A.6 F-023)
    InvalidAction(u8),         // Unknown action byte
    InvalidSide(u8),           // Unknown side byte
    SymbolNotFound(String),    // Multi-symbol: unknown symbol
    InconsistentState(String), // Generic state error
    CrossedQuote(i64, i64),    // bid > ask (if Error policy)
    LockedQuote(i64, i64),     // bid == ask (if Error policy)
    InvalidConfig(String),     // Config validation failure
    Generic(String),           // Catch-all
}
```

### Error Handling Philosophy

1. **Hard errors**: Invalid messages (when validation enabled)
2. **Soft errors**: Missing orders, price levels → tracked in `LobStats`
3. **Policy errors**: Crossed quotes → depends on `CrossedQuotePolicy`

### Checking for Issues

```rust
let stats = lob.stats();
if stats.has_warnings() {
    println!("Warnings: {}", stats.total_warnings());
    println!("  Cancel order not found: {}", stats.cancel_order_not_found);
    println!("  Trade order not found: {}", stats.trade_order_not_found);
}
```

---

## 8. Statistics and Analytics

### LobStats (src/lob/reconstructor.rs)

```rust
// Phase M M.A.4 (REV 3): #[non_exhaustive] applied — additive-only
// future evolution. External crates MUST construct via
// LobStats::default() + struct-update `..Default::default()`.
#[non_exhaustive]
pub struct LobStats {
    pub messages_processed: u64,
    pub system_messages_skipped: u64,
    pub active_orders: usize,
    pub bid_levels: usize,
    pub ask_levels: usize,
    // NOTE: Phase M M.A.4 REMOVED the `errors: u64` field (F-007 closure —
    // declared but never incremented). Specific anomaly counters below
    // expose the silent fall-through behavior previously hidden.
    pub crossed_quotes: u64,
    pub locked_quotes: u64,
    pub last_timestamp: Option<i64>,
    // Warning counters
    pub cancel_order_not_found: u64,
    pub cancel_price_level_missing: u64,
    pub cancel_order_at_level_missing: u64,
    pub trade_order_not_found: u64,
    pub trade_price_level_missing: u64,
    pub trade_order_at_level_missing: u64,
    // Phase M M.A.4 NEW (F-013 closure): observability counters for
    // silent fall-through paths. Increment BEFORE the recovery semantic
    // (modify_order falls through to add_order on missing id; add_order
    // falls through to modify_order on collision).
    pub modify_order_not_found: u64,
    pub add_order_id_collision: u64,
    pub book_clears: u64,
    pub noop_messages: u64,
}
```

### DayStats (src/statistics.rs)

Aggregates LOB state statistics over a trading day:

```rust
let mut day_stats = DayStats::new("2025-02-03");
let mut iter = loader.iter_messages_typed()?;  // preferred typed API
for msg_result in &mut iter {
    let state = lob.process_message(&msg_result?)?;
    day_stats.update(&state);
}
let stats = iter.finalize();  // clean-EOF vs torn-stream check: stats.is_clean_eof()
// Access: day_stats.mid_price.mean, day_stats.spread_bps.std(), etc.
```

> **Ingestion API note**: all examples in this document use `iter_messages_typed()`
> (yields `Result<MboMessage, BoundaryError>`). The older `iter_messages()` is
> `#[deprecated]` behind the default-on `legacy-iterator-api` feature — removal scheduled
> for the next MAJOR (0.3.0; calendar 2026-10-29). Do not write new code against it.

### Analytics (src/analytics.rs)

| Type | Purpose |
|------|---------|
| `DepthStats` | Per-side statistics (VWAP, volume distribution) |
| `MarketImpact` | Simulate order execution slippage |
| `LiquidityMetrics` | Combined bid/ask analysis |

---

## 9. Testing Patterns

### Unit Test Helper

```rust
fn create_test_message(
    order_id: u64,
    action: Action,
    side: Side,
    price_dollars: f64,
    size: u32,
) -> MboMessage {
    MboMessage::new(order_id, action, side, (price_dollars * NANODOLLARS_PER_DOLLAR_F64) as i64, size)
}
```

### Testing Crossed Quotes

```rust
#[test]
fn test_crossed_quote_policy_error() {
    let config = LobConfig::new(10)
        .with_crossed_quote_policy(CrossedQuotePolicy::Error)
        .with_logging(false);
    let mut lob = LobReconstructor::with_config(config);

    lob.process_message(&create_test_message(1, Action::Add, Side::Bid, 100.0, 100)).unwrap();
    let result = lob.process_message(&create_test_message(2, Action::Add, Side::Ask, 99.99, 200));

    assert!(matches!(result.unwrap_err(), TlobError::CrossedQuote(_, _)));
}
```

### Testing System Message Handling

```rust
#[test]
fn test_system_messages_skipped_by_default() {
    let mut lob = LobReconstructor::new(10);

    // Valid order first
    lob.process_message(&create_test_message(1, Action::Add, Side::Bid, 100.0, 100)).unwrap();

    // System message (order_id=0) - should be skipped, not error
    let msg = MboMessage::new(0, Action::Add, Side::Bid, 100_000_000_000, 100);
    lob.process_message(&msg).unwrap();  // No error!

    assert_eq!(lob.stats().system_messages_skipped, 1);
    assert_eq!(lob.order_count(), 1);  // Only the valid order
}
```

### Integration Test with Real Data

```rust
#[test]
fn test_with_real_data() {
    let loader = DbnLoader::new("path/to/data.dbn.zst")
        .expect("Failed to open")
        .skip_invalid(true);  // decode errors logged + counted instead of yielded

    let mut lob = LobReconstructor::new(10);
    let mut processed = 0u64;

    let mut iter = loader.iter_messages_typed().expect("Failed to open iterator");
    for msg_result in &mut iter {
        let msg = msg_result.expect("decode/convert boundary error");
        if let Ok(_state) = lob.process_message(&msg) {
            processed += 1;
        }
    }
    assert!(iter.finalize().is_clean_eof());  // torn-stream guard

    assert!(processed > 0);
    assert!(lob.stats().crossed_quotes < processed / 100);  // <1% crossed
}
```

---

## 10. Performance Considerations

### Target Performance

- **Throughput**: >1M messages/second (release mode)
- **Latency**: <10μs per message

### Optimization Techniques Used

1. **`#[inline]`**: Critical path functions
2. **`ahash`**: Faster than std HashMap
3. **`BTreeMap`**: O(1) best price access via cached values
4. **`PriceLevel` cached total**: O(1) aggregate size (no `values().sum()`)
5. **Cached best prices**: Avoid BTreeMap traversal on every message
6. **Stack-allocated `LobState`**: Fixed-size arrays, no heap allocation per snapshot
7. **`process_message_into()`**: Zero-allocation API for hot paths

### Benchmark Example

```rust
#[bench]
fn bench_process_message(b: &mut Bencher) {
    let mut lob = LobReconstructor::new(10);
    let msg = MboMessage::new(1, Action::Add, Side::Bid, 100_000_000_000, 100);
    b.iter(|| lob.process_message(&msg));
}
```

### Memory Efficiency

- `MboMessage`: 32 bytes (packed)
- `Order`: 16 bytes
- `LobState`: ~560 bytes (stack-allocated, 20 levels max)
  - Fixed arrays: 20×(8+4+8+4) = 480 bytes
  - Temporal fields + metadata: ~80 bytes

---

## 11. Common Patterns and Idioms

### Pattern: Processing a Day of Data

```rust
let loader = DbnLoader::new(path)?;
let mut lob = LobReconstructor::new(10);
let mut day_stats = DayStats::new(date);

let mut iter = loader.iter_messages_typed()?;
for msg_result in &mut iter {
    let state = lob.process_message(&msg_result?)?;
    day_stats.update(&state);
}
let stats = iter.finalize();
assert!(stats.is_clean_eof(), "torn DBN: mid_record_eof={}", stats.mid_record_eof);

// End of day
let norm_params = NormalizationParams::from_day_stats(&day_stats, 10);
norm_params.save_json("normalization.json")?;
```

### Pattern: Multi-Day Processing

```rust
for day_file in day_files {
    lob.full_reset();  // Clear state AND stats
    day_stats = DayStats::new(extract_date(&day_file));

    let mut iter = DbnLoader::new(&day_file)?.iter_messages_typed()?;
    for msg_result in &mut iter {
        let state = lob.process_message(&msg_result?)?;
        day_stats.update(&state);
    }
    assert!(iter.finalize().is_clean_eof());

    all_day_stats.push(day_stats);
}
```

**Important**: Use `full_reset()` between days, not `reset()`.

### Pattern: reset() vs full_reset()

| Method | Clears Book | Clears Stats | Use Case |
|--------|-------------|--------------|----------|
| `reset()` | ✅ | ❌ | Mid-session clear (Action::Clear) |
| `full_reset()` | ✅ | ✅ | New day/symbol/fresh start |

### Pattern: Custom Crossed Quote Handling

```rust
let config = LobConfig::new(10)
    .with_crossed_quote_policy(CrossedQuotePolicy::UseLastValid);

// Or handle manually:
let state = lob.process_message(&msg)?;
if state.is_crossed() {
    // Log or handle specially
}
```

---

## 12. Integration with Feature Extractor

This library is designed to work with [feature-extractor-MBO-LOB](https://github.com/nagarx/feature-extractor-MBO-LOB). The feature extractor **uses this library internally** for LOB reconstruction.

### Recommended: Use Feature Extractor Pipeline

The extractor is a **9-crate Cargo workspace** whose facade crate is `hft-extractor` (it
depends on this library via git tag `v0.2.1` + a monorepo `.cargo/config.toml` path
override). Its `Pipeline` handles LOB reconstruction internally; the production entry point
is the config-driven `export_dataset` CLI (`cargo run --release --features parallel --bin
export_dataset -- --config configs/<name>.toml`). Programmatically:

```rust
use hft_extractor::config::DatasetConfig;

let config = DatasetConfig::load_toml("configs/nvda_98feat.toml")?;
let layout = config.build_layout()?;
let mut pipeline = config.build_pipeline(&layout);

// Pipeline internally uses LobReconstructor (typed iterator ingestion)
let output = pipeline.process("data/NVDA.mbo.dbn.zst")?;
```

> **Archived-API note**: the `feature_extractor::prelude` / `PipelineBuilder` fluent API
> shown here in earlier revisions exists only in the extractor's archived monolith
> (`feature-extractor-MBO-LOB/archive/monolith-v1/` — historical reference, not compiled).

### Advanced: Manual Integration with Zero-Copy API

For custom processing or research, use the zero-allocation API:

```rust
use mbo_lob_reconstructor::{LobReconstructor, LobState, DbnLoader};

// Create reconstructor and reusable state buffer
let mut lob = LobReconstructor::new(10);
let mut state = LobState::new(10);  // Stack-allocated, reused across all messages

let loader = DbnLoader::new("data/NVDA.mbo.dbn.zst")?;

let mut iter = loader.iter_messages_typed()?;
for msg_result in &mut iter {
    let msg = msg_result?;  // BoundaryError::Decode/Convert propagates via ?

    // Zero-allocation: fills existing state buffer in-place
    lob.process_message_into(&msg, &mut state)?;

    // Access temporal information
    if let Some(delta_s) = state.delta_seconds() {
        let intensity = state.event_intensity().unwrap_or(0.0);
        // Use state.triggering_action, state.triggering_side, etc.
    }

    // State is ready for feature extraction
    if state.is_valid() {
        // Extract features from state...
    }
}
let stats = iter.finalize();
assert!(stats.is_clean_eof(), "torn DBN: mid_record_eof={}", stats.mid_record_eof);
```

### Key Integration Points

| This Library Provides | Feature Extractor Consumes |
|----------------------|---------------------------|
| `LobState` with temporal fields | LOB features (prices, sizes, spread) |
| `delta_ns`, `triggering_action` | Time-sensitive features (FI-2010 u6-u9) |
| `is_trade_event()`, `is_add_event()` | Event type classification |
| `microprice()`, `depth_imbalance()` | Derived microstructure features |

---

## 13. Known Limitations and Edge Cases

### Limitations

| Limitation | Description |
|------------|-------------|
| Single-threaded | `LobReconstructor` is not thread-safe |
| No persistence | State is in-memory only |
| Fixed precision | Prices are i64 fixed-point (9 decimal places) |
| Queue position separate | Use `QueuePositionTracker` for FIFO tracking (composable) |

### Edge Cases to Handle

1. **Order ID reuse**: Some exchanges reuse IDs → treated as modify
2. **Partial cancels**: Cancel with `size < order_size` reduces order
3. **Over-cancel**: Cancel with `size >= order_size` removes entirely
4. **Crossed at start of day**: Book may start crossed before first valid update
5. **Gap in sequence**: No sequence tracking beyond timestamp

### Data Quality Issues in Real Markets

```rust
// Typical stats from one day of NVDA data:
// messages_processed: 10,000,000
// system_messages_skipped: 1,393,000 (~14%)
// cancel_order_not_found: 50,000 (~0.5%) - Normal!
// crossed_quotes: 100 (~0.001%) - Normal!
```

See **`WARNINGS.md`** for the full `WarningCategory` taxonomy and the catalog of real-market data-quality edge cases (e.g. pre-market session start, partial-cancel handling) — the authoritative reference when triaging a preprocessing anomaly.

---

## 14. Composable Tracking Modules

These modules are **standalone and composable** - they do NOT modify the core `LobReconstructor`. Each processes `MboMessage` independently and can be used alongside or without LOB reconstruction.

### Design Philosophy

```
┌─────────────────────────────────────────────────────────────────────┐
│                        MboMessage Stream                             │
└─────────────────────────────────────────────────────────────────────┘
         │              │              │              │
         ▼              ▼              ▼              ▼
┌─────────────┐  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐
│   LOB       │  │   Queue     │  │   Order     │  │   Trade     │
│ Reconstruct │  │  Position   │  │  Lifecycle  │  │ Aggregator  │
└─────────────┘  └─────────────┘  └─────────────┘  └─────────────┘
         │              │              │              │
         ▼              ▼              ▼              ▼
     LobState     QueuePosition    Lifecycle     Trade + Side
```

### QueuePositionTracker

Tracks FIFO queue position of orders at each price level. Critical for execution probability models.

```rust
use mbo_lob_reconstructor::{QueuePositionTracker, QueuePositionConfig};

let mut tracker = QueuePositionTracker::new(QueuePositionConfig::default());

for msg in messages {
    tracker.process_message(&msg);
    
    // Query queue position for a specific order
    if let Some(info) = tracker.queue_position(msg.order_id) {
        println!("Order {} at position {} with {} volume ahead",
                 msg.order_id, info.position, info.volume_ahead);
    }
}

// Aggregate statistics
let stats = tracker.stats();
println!("Orders tracked: {}, removed: {}", stats.orders_tracked, stats.orders_removed);
println!("Cancel not found: {}, Fill not found: {}", stats.cancel_not_found, stats.fill_not_found);
```

**Key Methods:**
- `queue_position(order_id) -> Option<QueuePositionInfo>` — FIFO position, volume ahead, queue length
- `volume_ahead(order_id) -> Option<u64>` — volume ahead of specific order
- `queue_length(side, price) -> usize` — orders at a price level
- `level_volume(side, price) -> u64` — total volume at a price level
- `active_orders() -> usize` — total tracked orders
- `best_level_imbalance() -> Option<(f64, u64, u64)>` — BBO queue imbalance
- `multi_level_imbalance(levels) -> Option<(f64, u64, u64)>` — multi-level imbalance
- `average_queue_position(side) -> Option<f64>` — average position across all levels
- `recent_position_changes() -> &VecDeque<PositionChange>` — if tracking enabled
- `stats() -> &QueueStats` — tracking statistics
- `reset()` — clear all state

### OrderLifecycleTracker

Tracks orders through Add → Modify* → Cancel|Fill lifecycle.

```rust
use mbo_lob_reconstructor::{OrderLifecycleTracker, OrderLifecycleConfig, LifecycleEvent};

let config = OrderLifecycleConfig::default();
let mut tracker = OrderLifecycleTracker::new(config);

for msg in messages {
    if let Some(event) = tracker.process_message(&msg) {
        match event {
            LifecycleEvent::Created(lc) => {
                println!("New order {} at ${:.2}", lc.order_id, lc.original_price as f64 / NANODOLLARS_PER_DOLLAR_F64);
            }
            LifecycleEvent::Modified { order_id, modification } => {
                println!("Order {} modified: {:?}", order_id, modification);
            }
            LifecycleEvent::PartialFill { order_id, fill_size, .. } => {
                println!("Order {} partial fill: {} shares", order_id, fill_size);
            }
            LifecycleEvent::Completed(lc) => {
                println!("Order {} completed in {:?}", lc.order_id, lc.time_alive_ns());
            }
        }
    }
}

// Query active order by ID
if let Some(lifecycle) = tracker.get_active(order_id) {
    println!("Order {} alive for {:?} ns", lifecycle.order_id, lifecycle.time_alive_ns());
}

// Statistics
let stats = tracker.stats();
println!("Observed: {}, Inferred: {}", stats.observed_orders, stats.inferred_orders);
println!("Overfills detected: {}", stats.overfill_count);

// Active order features (for ML)
let features = tracker.active_order_features();
println!("Active orders: {}, Avg age: {:?}", features.total_count, features.avg_age_ns);
```

**Key Insight**: Handles mid-session data starts by inferring lifecycles for pre-existing orders (marked with `OrderOrigin::Inferred`).

### DayBoundaryDetector

Detects trading day boundaries for proper train/test splits and state resets.

```rust
use mbo_lob_reconstructor::{DayBoundaryDetector, DayBoundaryConfig};

let config = DayBoundaryConfig::us_equity();  // 9:30 AM - 4:00 PM ET
let mut detector = DayBoundaryDetector::new(config);

for msg in messages {
    if let Some(ts) = msg.timestamp {
        if let Some(boundary) = detector.check_boundary(ts) {
            println!("Day {} ended, day {} started",
                     boundary.previous_day_index,
                     boundary.new_day_index);
            
            // Reset your trackers here
            lob.full_reset();
            queue_tracker.reset();
            lifecycle_tracker.reset();
        }
    }
}

// Query current day info
let day_idx = detector.current_day_index();
let day_stats = detector.current_day_stats();
println!("Day {}: {} messages, {} trades", day_idx, day_stats.messages, day_stats.trades);
println!("Boundaries detected so far: {}", detector.boundaries_detected());
```

### TradeAggregator

Aggregates fill events into trades with aggressor side detection.

```rust
use mbo_lob_reconstructor::{TradeAggregator, TradeAggregatorConfig};

let mut aggregator = TradeAggregator::new(TradeAggregatorConfig::default());

for msg in messages {
    if let Some(trade) = aggregator.process_message(&msg) {
        println!("Trade: {} shares @ ${:.2} (aggressor: {:?})",
                 trade.size, trade.price_f64(), trade.aggressor_side);
    }
}

// IMPORTANT: flush() to get the last pending trade (aggregation window may hold one)
if let Some(last_trade) = aggregator.flush() {
    println!("Final trade: {} shares", last_trade.size);
}

// Trade imbalance (buy pressure vs sell pressure)
let imbalance = aggregator.trade_imbalance();  // Range: [-1.0, 1.0]
println!("Buy pressure: {:.1}%", (imbalance + 1.0) / 2.0 * 100.0);

// Recent trades for analysis
for trade in aggregator.recent_trades() {
    // ...
}
```

**Aggressor Detection Logic:**
- Trade against **bid** order → aggressor is **seller**
- Trade against **ask** order → aggressor is **buyer**

### Composing All Trackers

```rust
use mbo_lob_reconstructor::{
    LobReconstructor, QueuePositionTracker, OrderLifecycleTracker,
    DayBoundaryDetector, TradeAggregator,
    QueuePositionConfig, OrderLifecycleConfig, DayBoundaryConfig, TradeAggregatorConfig,
};

// Initialize all trackers
let mut lob = LobReconstructor::new(10);
let mut queue_tracker = QueuePositionTracker::new(QueuePositionConfig::default());
let mut lifecycle_tracker = OrderLifecycleTracker::new(OrderLifecycleConfig::default());
let mut day_detector = DayBoundaryDetector::new(DayBoundaryConfig::us_equity());
let mut trade_aggregator = TradeAggregator::new(TradeAggregatorConfig::default());

// Process messages through all trackers
for msg in messages {
    // Check for day boundary first
    if let Some(ts) = msg.timestamp {
        if let Some(_boundary) = day_detector.check_boundary(ts) {
            lob.full_reset();
            queue_tracker.reset();
            lifecycle_tracker.reset();
            trade_aggregator.reset();
        }
    }
    
    // Process through each tracker
    let state = lob.process_message(&msg)?;
    queue_tracker.process_message(&msg);
    lifecycle_tracker.process_message(&msg);
    trade_aggregator.process_message(&msg);
    
    // Now you have:
    // - state: LobState with temporal fields
    // - queue_tracker.queue_position(order_id): Queue position info
    // - lifecycle_tracker.get_active(order_id): Order lifecycle
    // - trade_aggregator.trade_imbalance(): Buy/sell pressure
}
```

---

## Quick Reference

### Imports for Common Tasks

```rust
// Basic reconstruction
use mbo_lob_reconstructor::{LobReconstructor, MboMessage, LobState, Action, Side};

// With configuration
use mbo_lob_reconstructor::{LobReconstructor, LobConfig, CrossedQuotePolicy};

// File loading (requires "databento" feature)
use mbo_lob_reconstructor::DbnLoader;

// Constants
use mbo_lob_reconstructor::constants::{NANODOLLARS_PER_DOLLAR_F64, NS_PER_SECOND_F64};

// Statistics
use mbo_lob_reconstructor::{DayStats, RunningStats, NormalizationParams};

// Analytics
use mbo_lob_reconstructor::{DepthStats, MarketImpact, LiquidityMetrics};

// Warnings
use mbo_lob_reconstructor::{WarningTracker, WarningCategory, Warning};

// Queue Position Tracking
use mbo_lob_reconstructor::{QueuePositionTracker, QueuePositionConfig, QueuePositionInfo};

// Order Lifecycle Tracking
use mbo_lob_reconstructor::{OrderLifecycleTracker, OrderLifecycleConfig, OrderLifecycle, LifecycleEvent};

// Day Boundary Detection
use mbo_lob_reconstructor::{DayBoundaryDetector, DayBoundaryConfig, DayBoundary};

// Trade Aggregation
use mbo_lob_reconstructor::{TradeAggregator, TradeAggregatorConfig, Trade, Fill};
```

### Price Conversion

```rust
use mbo_lob_reconstructor::constants::{NANODOLLARS_PER_DOLLAR, NANODOLLARS_PER_DOLLAR_F64};

// Dollars to fixed-point
let price_fixed: i64 = (price_dollars * NANODOLLARS_PER_DOLLAR_F64) as i64;

// Fixed-point to dollars
let price_dollars: f64 = price_fixed as f64 / NANODOLLARS_PER_DOLLAR_F64;
```

### Checking Book Health

```rust
let state = lob.process_message(&msg)?;

// Validity checks
state.is_valid()      // Has both bid and ask
state.is_consistent() // bid < ask
state.is_crossed()    // bid > ask (invalid)
state.is_locked()     // bid == ask (unusual)

// Stats checks
lob.stats().has_warnings()
lob.stats().total_warnings()
```

---

## 15. Parquet Export Module (feature: `export`)

### Overview

The `export` module provides a feature-gated Parquet export for raw LOB snapshots and MBO events. It writes data that has **not** been transformed by sampling, normalization, or labeling, making it suitable for unbiased statistical analysis in the `MBO-LOB-analyzer` Python repository.

### Dependencies

- `arrow = "55"` and `parquet = "55"` (MSRV 1.81, compatible with this project's `rust-version = "1.82"`)
- Feature-gated: only compiled when `--features export` is specified

### Module Structure

| File | Purpose |
|------|---------|
| `export/mod.rs` | `ExportConfig`, `DownsampleConfig`, `DownsampleStrategy`, `ParquetExportStats`, `DownsampleStats` (Phase O B.3 out-of-order observability) |
| `export/schema.rs` | `lob_snapshot_schema()`, `mbo_event_schema()` — single source of truth for column definitions |
| `export/batch.rs` | `LobBatch`, `MboBatch` — column-oriented buffers that convert to Arrow `RecordBatch` |
| `export/lob_writer.rs` | `LobSnapshotWriter` — buffers `LobState` and writes Parquet row groups |
| `export/mbo_writer.rs` | `MboEventWriter` — buffers `MboMessage` and writes Parquet row groups |
| `bin/export_to_parquet.rs` | CLI binary for batch DBN-to-Parquet conversion |

### LOB Snapshot Schema

| Column | Type | Nullable | Description |
|--------|------|----------|-------------|
| `timestamp_ns` | Int64 | No | Nanoseconds since epoch |
| `sequence` | UInt64 | No | Message sequence number |
| `levels` | UInt8 | No | Active level count |
| `best_bid` | Int64 | Yes | Best bid in nanodollars |
| `best_ask` | Int64 | Yes | Best ask in nanodollars |
| `bid_prices` | FixedSizeList(Int64, N) | No | Bid prices per level |
| `bid_sizes` | FixedSizeList(UInt32, N) | No | Bid sizes per level |
| `ask_prices` | FixedSizeList(Int64, N) | No | Ask prices per level |
| `ask_sizes` | FixedSizeList(UInt32, N) | No | Ask sizes per level |
| `delta_ns` | UInt64 | No | Nanoseconds since previous update |
| `triggering_action` | UInt8 | Yes | Action enum byte |
| `triggering_side` | UInt8 | Yes | Side enum byte |
| `mid_price` | Float64 | Yes | Derived: (bid+ask)/2 in dollars |
| `spread` | Float64 | Yes | Derived: ask-bid in dollars |
| `spread_bps` | Float64 | Yes | Derived: spread in basis points |
| `microprice` | Float64 | Yes | Derived: volume-weighted mid |
| `total_bid_volume` | UInt64 | No | Derived: sum of bid sizes |
| `total_ask_volume` | UInt64 | No | Derived: sum of ask sizes |
| `depth_imbalance` | Float64 | Yes | Derived: (bid_vol-ask_vol)/(bid_vol+ask_vol) |
| `book_consistency` | UInt8 | No | Derived: 0=Valid, 1=Empty, 2=Locked, 3=Crossed |

Derived columns are included when `ExportConfig::include_derived = true` (default).

### MBO Event Schema

| Column | Type | Nullable |
|--------|------|----------|
| `timestamp_ns` | Int64 | Yes |
| `order_id` | UInt64 | No |
| `action` | UInt8 | No |
| `side` | UInt8 | No |
| `price` | Int64 | No |
| `size` | UInt32 | No |

### File Metadata

Both Parquet files embed key-value metadata in the footer:

- `schema_version`: "1.0"
- `source`: "mbo-lob-reconstructor"
- `reconstructor_version`: from Cargo.toml
- `price_unit`: "nanodollars"
- `size_unit`: "shares"
- `timestamp_unit`: "nanoseconds_since_epoch"
- `lob_levels`: (LOB files only) number of exported levels
- `date`, `symbol`: when provided via extra metadata

### Configuration

```rust
ExportConfig {
    levels: usize,              // LOB levels (default: 10, max: 20)
    include_derived: bool,       // mid_price, spread, etc. (default: true)
    include_mbo_events: bool,    // also export MBO events (default: true)
    batch_size: usize,           // rows per row group (default: 65536)
    compression: Compression,    // Snappy (default) or Uncompressed
    downsample: Option<DownsampleConfig>,
}
```

### Downsampling Strategies

| Strategy | Description |
|----------|-------------|
| `DownsampleStrategy::None` | Export every snapshot |
| `DownsampleStrategy::EveryN(n)` | Export every N-th snapshot |
| `DownsampleStrategy::MinIntervalNs(ns)` | At most one snapshot per N nanoseconds |

### Data Volume Estimates (NVDA, per day)

| Dataset | Rows | Uncompressed | Snappy |
|---------|------|-------------|--------|
| MBO events | ~7M | ~392 MB | ~80-120 MB |
| LOB snapshots (all) | ~7M | ~2.8 GB | ~400-600 MB |
| LOB snapshots (EveryN(100)) | ~70K | ~28 MB | ~6 MB |

### Testing

The export module is covered by inline unit tests in `schema.rs` (schema construction,
metadata, nullability) plus integration tests in `tests/export_test.rs` (round-trip, edge
cases, batching, downsampling incl. the Phase O B.3 out-of-order suite, metadata, numerical
precision). Counts are intentionally not hand-maintained here (hft-rules §11) — run
`cargo test --features "databento export" 2>&1 | grep "test result"` for the live count.

---

*Last updated: 2026-07-07 (Phase-2 doc-truth pass: typed-iterator ingestion API coverage + live extractor-workspace integration sections; content baseline 2026-04-30, post Phase M REV 3 — Boundary Discipline cycle)*
*Crate version: 0.2.1*

