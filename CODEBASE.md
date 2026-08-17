# MBO-LOB-Reconstructor: Codebase Technical Reference

> **Purpose**: This document provides complete technical details for LLMs and developers to understand, modify, and extend the codebase without prior context.

> **Pipeline scope (2026-06-02).** This module is part of an **intraday trading research pipeline** — an experiment-first platform for discovering and validating *any* profitable **intraday** trading edge (no overnight positions), across approach classes (microstructure/HFT, scalping, intraday momentum, intraday statistical arbitrage, …) and instruments (equities, futures, same-day options). The pipeline *originated* as a high-frequency NVDA MBO/LOB microstructure system — that origin explains the "HFT" / "LOB" / "MBO" naming here — and that microstructure-direction program is now one (largely-closed) track among many. **Names are historical; the mission is general.** This module's role: the Rust ingestion front-end — reconstructs limit-order-book state (`LobState`) from raw Market-By-Order `.dbn.zst` events (~1M msg/s; **BBO accuracy CORRECTED 2026-08-01: the long-quoted "99.17%" is not in its own source artifact — `data/validation_results_july2025.json` reports best-price exact match 95.56% bid / 95.73% ask and best-size exact match 83.66% / 83.06%. See `WARNINGS.md` §Validation Results Summary**); the order-book source feeding feature extraction. For the full mission + approach taxonomy + capability-readiness boundary, see root `CLAUDE.md` §Research Scope & Charter (+ `CROSS_ASSET_OFI_FINDINGS_AND_ISSUES_2026_06_01.md` §9).

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
| System-shaped filtering | Internal predicate `order_id == 0 || size == 0 || price <= 0`; Clear exempt. This is not a universal DBN heartbeat taxonomy and affects true-Trade coverage on the bounded NVDA/XNAS feature path. |
| Crossed Quote Handling | Configurable policies for bid ≥ ask |
| Temporal Fields | Time delta, triggering action/side (FI-2010 u6-u9) |
| Analytics | Microprice, VWAP, depth imbalance, market impact |
| Statistics | Welford's algorithm for streaming mean/std |
| Multi-Symbol | Manage multiple LOBs simultaneously |
| Queue Position Tracking | FIFO position, volume ahead (composable module) |
| Order Lifecycle Tracking | Add→Modify→Cancel/Fill lifecycle (composable module) |
| Day Boundary Detection | Trading day boundaries for train/test splits |
| Trade Aggregation | Fill-only helper with aggressor-side inversion. It is not safe on current `DbnLoader` output: the v0.3.0 bridge merges wire `T` (aggressor side) and `F` (resting side), while `TradeAggregator` reverses side for both. Use only with independently supplied, resting-side Fill semantics. |
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
| `lob/trade_aggregator` | Fill-only aggregation helper; unsafe on current bridge output because it cannot distinguish wire Trade from Fill and reverses side unconditionally | `TradeAggregator`, `Trade`, `Fill` |
| `source` | Provider abstraction | `MarketDataSource`, `SourceMetadata`, `DbnSource`, `VecSource` |
| `hotstore` | Decompressed data caching | `HotStoreConfig`, `HotStoreManager` |
| `loader` | DBN file streaming | `DbnLoader`, `TypedMessageIterator` (preferred: `iter_messages_typed()` -> `Result<MboMessage, BoundaryError>`), `BoundaryError`, `LoaderStats`, `MessageIterator` (legacy, `#[deprecated]`, default-on and still present in v0.3.0; calendar removal trigger 2026-10-29) |
| `dbn_bridge` | DBN → internal conversion | `DbnBridge` |
| `statistics` | ML statistics | `RunningStats`, `DayStats`, `NormalizationParams` |
| `analytics` | Market microstructure | `DepthStats`, `MarketImpact`, `LiquidityMetrics` |
| `warnings` | Issue tracking | `WarningTracker`, `WarningTrackerConfig`, `Warning`, `WarningCategory`, `WarningSummary` |
| `export` | Parquet export (feature-gated) | `ExportConfig`, `LobSnapshotWriter`, `MboEventWriter`, `ParquetExportStats` |

---

## 3. Core Types and Data Structures

### MboMessage (src/types.rs)

Input message representing one event after the DBN bridge. It is narrower than
`dbn::MboMsg` and must not be treated as a lossless wire mirror.

```rust
pub struct MboMessage {
    pub order_id: u64,          // Venue order ID; zero is not universally a heartbeat
    pub action: Action,         // Internal action after bridge conversion
    pub side: Side,             // Bid, Ask, None
    pub price: i64,             // Raw fixed-point; sentinel-check, then / 1e9
    pub size: u32,              // Instrument/publisher-native quantity
    pub timestamp: Option<i64>, // Current bridge copies hd.ts_event
}
```

`DbnBridge` accepts `ts_event == 0` on the internal structural shape
(`order_id == 0 || size == 0 || price <= 0`) and emits `timestamp: None` so the
message remains observable. `InvalidTimestamp(0)` is reserved for a zero
timestamp on a non-structural/order-shaped row; a value above `i64::MAX` is
always rejected after the checked signed-domain test.

**Boundary facts**:

- `MboMessage::is_system_message()` implements
  `order_id == 0 || size == 0 || price <= 0`. That is an internal structural
  filter, not a DBN heartbeat/status type test. `Action::Clear` is exempted by
  the reconstructor and extractor.
- DBN MBO's primary/index timestamp is `ts_recv`; common-header `hd.ts_event`
  is matching-engine-received time. Current v0.3.0 `DbnBridge` stores
  `hd.ts_event` and drops `ts_recv`.
- The fixed-price 1e9 divisor produces an instrument-native price. The current
  equity consumers interpret it as USD; quantities are shares only under that
  dataset/instrument context.

### Action Enum

```rust
pub enum Action {
    Add = b'A',      // New order
    Modify = b'M',   // Existing order price and/or size changed
    Cancel = b'C',   // Existing order fully or partially cancelled
    Trade = b'T',    // Wire T: aggressor side; DBN says no book effect
    Fill = b'F',     // Wire F: resting side; DBN says no book effect
    Clear = b'R',    // Reset all orders for the instrument
    None = b'N',     // No book effect; may carry other information
}
```

`Action::from_byte()` distinguishes `T` and `F`. The current DBN bridge does
not: `b'T' | b'F' => Action::Trade` remains live in v0.3.0. Do not copy that
current limitation into a statement of vendor semantics.

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

### Structural Filtering (Step 1)

Current `process_message_into()` behavior is:

```rust
if self.config.skip_system_messages
    && msg.is_system_message()
    && msg.action != Action::Clear
{
    self.stats.system_messages_skipped += 1;
    // Temporal output is still populated before the early return.
    return Ok(());
}
```

The predicate is a crate-local structural heuristic, not a DBN system-record
classification. The Clear exemption is load-bearing because a valid wire `R`
record is zero-shaped. FINDING-122 additionally shows that, on the bounded
NVDA/XNAS feature path, `order_id == 0` selects true Trades; upstream use of
this filter therefore changes trade coverage.

### Action Processing (Step 3)

| Internal action | Current v0.3.0 logic | DBN semantic boundary |
|---|---|---|
| **Add** | Insert order into price level and orders map | Book-affecting |
| **Modify** | Remove old order, add new; handles price change | Book-affecting |
| **Cancel** | Reduce size or remove order | Book-affecting |
| **Trade/Fill** | Both route to `process_trade()` and mutate the book | Known implementation limitation: DBN documents wire `T` and `F` as no-book-effect and gives them opposite side conventions |
| **Clear** | `reset()`, increment `book_clears` | Wire `R`: clear all orders; explicitly exempt from structural filter/validation |
| **None** | No-op, increment `noop_messages` | No book effect |

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
    InvalidTimestamp(i64),     // ts_event > i64::MAX, or ts_event == 0 on a non-structural/order-shaped row
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

> **Ingestion API note**: examples use `iter_messages_typed()` (yielding
> `Result<MboMessage, BoundaryError>`). The older `iter_messages()` is
> `#[deprecated]` behind the default-on `legacy-iterator-api` feature and is
> still present in v0.3.0. Its calendar removal trigger is 2026-10-29. Do not
> write new code against it.

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

- `MboMessage`: 40 bytes on the audited target (48 under source-order layout is
  possible); `repr(Rust)` and no exact-size assertion mean this is not packed or
  ABI-stable
- `Order`: 16 bytes
- `LobState`: 576 bytes on the audited target (stack-allocated, 20 levels max);
  the live test only bounds it between 501 and 699 bytes
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

The extractor is a **9-crate Cargo workspace** whose facade crate is
`hft-extractor`; it depends on this library at git tag `v0.3.0` with a
monorepo `.cargo/config.toml` path override. Its `Pipeline` handles LOB
reconstruction internally; the production entry point is the config-driven
`export_dataset` CLI (`cargo run --release --features parallel --bin
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
// cancel_order_not_found: 50,000 (~0.5%) - NOT normal. See correction below.
// crossed_quotes: 100 (~0.001%) - Normal!
```

> ⚠️ **CORRECTION 2026-08-01 — `cancel_order_not_found` / `trade_order_not_found` are NOT normal
> market properties.** OLD claim: "~0.5% — Normal!". NEW: **100% of the mass is an artifact of the
> `b'T' | b'F' => Action::Trade` merge at `src/dbn_bridge.rs:125`.** Evidence: 473,410/473,410
> (100.000%) of `F` rows on 2025-02-03 are followed by a `C` carrying an identical
> order_id/size/timestamp/side — the `F` has already deleted (full fill) or exhausted (partial fill)
> the order, so the paired `C` finds nothing. A corrected replay that treats `F` as a book no-op
> gives **exactly zero** on both counters, on two independent days:
> `cancel_order_not_found` 393,790 → **0** and `trade_order_not_found` 33,293 → **0** (2025-02-03);
> 261,386 → **0** and 18,061 → **0** (2025-07-01).
> **These five counters are therefore the free acceptance test for the pending decoder fix: they must
> read EXACTLY 0 afterwards, not "≈0" or "reduced".** (Corollary: the 2026-04 backbone audit §3.5
> attributed a residual BBO mismatch to `cancel_order_not_found` "upstream data quality" — that
> attribution is falsified, since the counter is itself 100% this bug.)

> ✅ **UPDATE 2026-08-17 — THE FIX LANDED, AND THE "five counters" ACCEPTANCE SET WAS WRONG.**
> COMMIT 1 (decode split) + COMMIT 2a (router: `Fill` is no longer routed through `process_trade`)
> are on `claude/backbone-v5-reconstructor` **`c9c6f60`**. ⚠️ **`main` does NOT carry them** — the
> block above still describes what production emits.
> **What fired, and is a real measurement:** `cancel_order_not_found` XNAS 261,386 → **0** (07-01)
> and 207,959 → **0** (07-02); ARCX 157,493 → **0** and 127,527 → **0**; plus
> `modify_order_not_found` 369 → **0** / 324 → **0** on ARCX, a path invisible on XNAS (which emits
> zero `M` bytes). `cancel_order_not_found` is the **PRIMARY** channel because its code path
> **survives** the commit, so reaching 0 is a measurement rather than a removal.
> 🔴 **The three `trade_*` counters are NOT valid channels and must be dropped from any acceptance
> set (KNOWN-WRONG row N1).** After the commit they have **zero increment sites in production
> code** — they read 0 on any data, any venue, forever, and a deliberately-built *impostor* fix
> scores 0 on them too. The `33,293 → 0` / `18,061 → 0` figures above are retained as the historical
> record of the defect, **never** as a post-fix target.
> ⚠️ **Nor was that mass "signal" (row N2)**: it was itself an artifact of the double-decrement, so
> the predicted transfer into a new `fill_referenced_unknown_order` counter reads **0**.
> `Fill` was **repointed, not deleted** — it now CHECKS without mutating, yielding 556,278 vendor
> assertions over two days at 100.000% conformance on existence, side and sufficient resting size.
> Full detail: `WARNINGS.md` § ORDER_NOT_FOUND.

> **FINDING-122 interpretation boundary.** The raw-tape consequence and the
> current feature-path consequence are different. On raw NVDA/XNAS MBO, merging
> `T` (aggressor side) and `F` (resting side) annihilates signed direction. In
> the current feature-extractor path, the separate
> `is_system_message(order_id == 0)` filter drops exactly true Trades and leaves
> Fills, whose resting-side convention is the convention the feature code
> assumes. The two defects therefore cancel for sign on that path; they cost
> coverage and do **not** reopen the registered direction closures. This claim
> is limited to the measured current NVDA/XNAS producer path, says nothing about
> direct raw-tape consumers, and becomes historical when producer behavior
> changes.

See **`WARNINGS.md`** for the full `WarningCategory` taxonomy and the catalog of real-market data-quality edge cases (e.g. pre-market session start, partial-cancel handling) — the authoritative reference when triaging a preprocessing anomaly.

---

## 14. Composable Tracking Modules

These modules are **standalone and composable** - they do NOT modify the core `LobReconstructor`. Their APIs and filters are distinct: queue-position and lifecycle trackers consume `MboMessage` and reject the internal structural shape plus `Side::None`; `TradeAggregator` consumes `MboMessage` but has no structural-shape filter; `DayBoundaryDetector` uses `check_boundary(timestamp)` followed by `record_message(...)`, not `process_message`. They can be used alongside or without LOB reconstruction only when those separate contracts are honored.

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

Aggregates resting-side Fill events into trades by reversing the supplied side to
derive aggressor side. This helper is currently unused by the pipeline and is
**unsafe on direct `DbnLoader` / `DbnBridge` output**: v0.3.0 maps both wire `T`
(whose side is already aggressor side) and wire `F` (resting side) to
`Action::Trade`, while `TradeAggregator::process_message()` reverses side for
every `Action::Trade | Action::Fill`. The example below is valid only when the
caller has independently selected true Fill rows and preserved resting-side
semantics; it must not be connected to the current bridge stream.

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

### Composing Reconstruction, Queue/Lifecycle Tracking, and Day Boundaries

The queue and lifecycle trackers can share a current bridge stream with the
reconstructor, and the day detector consumes the timestamp/statistics projection.
`TradeAggregator` is intentionally excluded from this current-bridge example;
see its Fill-only precondition above.

```rust
use mbo_lob_reconstructor::{
    Action, LobReconstructor, QueuePositionTracker, OrderLifecycleTracker,
    DayBoundaryDetector,
    QueuePositionConfig, OrderLifecycleConfig, DayBoundaryConfig,
};

// Initialize all trackers
let mut lob = LobReconstructor::new(10);
let mut queue_tracker = QueuePositionTracker::new(QueuePositionConfig::default());
let mut lifecycle_tracker = OrderLifecycleTracker::new(OrderLifecycleConfig::default());
let mut day_detector = DayBoundaryDetector::new(DayBoundaryConfig::us_equity());

// Process messages through all trackers
for msg in messages {
    // Check for day boundary first
    if let Some(ts) = msg.timestamp {
        if let Some(_boundary) = day_detector.check_boundary(ts) {
            lob.full_reset();
            queue_tracker.reset();
            lifecycle_tracker.reset();
        }
    }
    
    // Process through each tracker
    let state = lob.process_message(&msg)?;
    queue_tracker.process_message(&msg);
    lifecycle_tracker.process_message(&msg);
    day_detector.record_message(
        msg.timestamp,
        matches!(msg.action, Action::Trade | Action::Fill),
        msg.size,
    );
    
    // Now you have:
    // - state: LobState with temporal fields
    // - queue_tracker.queue_position(order_id): Queue position info
    // - lifecycle_tracker.get_active(order_id): Order lifecycle
    // - day_detector.current_day_stats(): Per-day message/trade statistics
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

// Fill-only Trade Aggregation (do not feed current T/F-merged DbnBridge output)
use mbo_lob_reconstructor::{TradeAggregator, TradeAggregatorConfig, Trade, Fill};
```

### Price Conversion

```rust
use mbo_lob_reconstructor::constants::{NANODOLLARS_PER_DOLLAR, NANODOLLARS_PER_DOLLAR_F64};

// Instrument-native price -> DBN-style fixed-point.
let price_fixed: i64 = (price_native * NANODOLLARS_PER_DOLLAR_F64) as i64;

// Fixed-point -> instrument-native price, after checking the applicable sentinel.
let price_native: f64 = price_fixed as f64 / NANODOLLARS_PER_DOLLAR_F64;
```

The constant names are historical and equity-oriented. Division by 1e9 is the
DBN storage scale; it does not by itself establish USD. Current XNAS/ARCX equity
callers may label `price_native` as dollars after instrument context is known.

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

The `export` module provides feature-gated Parquet output for reconstructed LOB
snapshots and converted MBO projections. Snapshot rows are accepted, valid
states and may be downsampled or rejected by ordering checks. MBO rows are not
downsampled after successful bridge conversion, but are a lossy six-field
projection: the bridge uses `hd.ts_event`, merges wire `T` and `F`, and omits
publisher/instrument identity, `ts_recv`, flags, channel ID, and
`ts_in_delta`. Neither file is vendor-raw DBN or an automatically unbiased
analysis population; the applicable filters and denominators must be stated.

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
- `price_unit`: `"nanodollars"` (current equity-oriented crate metadata; the generic DBN rule is fixed raw / 1e9 -> instrument-native price)
- `size_unit`: `"shares"` (current equity export contract; not a universal DBN quantity unit)
- `timestamp_unit`: `"nanoseconds_since_epoch"` (current values come from bridge-mapped `hd.ts_event`, not MBO primary `ts_recv`)
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
*Crate version: 0.3.0*
