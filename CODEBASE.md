# MBO-LOB-Reconstructor: Codebase Technical Reference

> **Purpose**: This document provides complete technical details for LLMs and developers to understand, modify, and extend the codebase without prior context.

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
| Analytics | Microprice, VWAP, depth imbalance, market impact |
| Statistics | Welford's algorithm for streaming mean/std |
| Multi-Symbol | Manage multiple LOBs simultaneously |
| DBN Support | Native Databento file loading |

### Directory Structure

```
src/
├── lib.rs              # Public API, re-exports
├── types.rs            # MboMessage, LobState, Action, Side
├── error.rs            # TlobError, Result type
├── lob/
│   ├── mod.rs          # Module overview
│   ├── reconstructor.rs # LobReconstructor core logic
│   └── multi_symbol.rs # MultiSymbolLob manager
├── loader.rs           # DbnLoader for file I/O
├── dbn_bridge.rs       # Databento format conversion
├── statistics.rs       # RunningStats, DayStats, NormalizationParams
├── analytics.rs        # DepthStats, MarketImpact, LiquidityMetrics
└── warnings.rs         # WarningTracker, WarningCategory
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
│                      types.rs + error.rs                         │
│              (MboMessage, LobState, TlobError)                   │
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
| `types` | Data structures, no logic | `MboMessage`, `LobState`, `Action`, `Side` |
| `error` | Error definitions | `TlobError`, `Result<T>` |
| `lob/reconstructor` | Core LOB reconstruction | `LobReconstructor`, `LobConfig`, `LobStats` |
| `lob/multi_symbol` | Multi-stock management | `MultiSymbolLob`, `MultiSymbolStats` |
| `loader` | DBN file streaming | `DbnLoader`, `MessageIterator` |
| `dbn_bridge` | DBN → internal conversion | `DbnBridge` |
| `statistics` | ML statistics | `RunningStats`, `DayStats`, `NormalizationParams` |
| `analytics` | Market microstructure | `DepthStats`, `MarketImpact`, `LiquidityMetrics` |
| `warnings` | Issue tracking | `WarningTracker`, `Warning`, `WarningCategory` |

---

## 3. Core Types and Data Structures

### MboMessage (src/types.rs)

Input message representing a single order book event.

```rust
pub struct MboMessage {
    pub order_id: u64,        // Unique order identifier (0 = system message)
    pub action: Action,       // Add, Modify, Cancel, Trade, Fill, Clear, None
    pub side: Side,           // Bid, Ask, None
    pub price: i64,           // Fixed-point: divide by 1e9 for dollars
    pub size: u32,            // Shares/contracts
    pub timestamp: Option<i64>, // Nanoseconds since epoch
}
```

**Critical**: Messages with `order_id=0`, `size=0`, or `price<=0` are **system messages** (heartbeats, status updates), not valid orders.

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

Output snapshot of the order book at N price levels.

```rust
pub struct LobState {
    pub bid_prices: Vec<i64>,  // Highest to lowest
    pub bid_sizes: Vec<u32>,   // Aggregated at each price
    pub ask_prices: Vec<i64>,  // Lowest to highest
    pub ask_sizes: Vec<u32>,   // Aggregated at each price
    pub best_bid: Option<i64>, // Cached best prices
    pub best_ask: Option<i64>,
    pub levels: usize,         // Number of levels tracked
    pub timestamp: Option<i64>,
    pub sequence: u64,         // Message sequence number
}
```

### LobReconstructor Internal State (src/lob/reconstructor.rs)

```rust
pub struct LobReconstructor {
    config: LobConfig,
    bids: BTreeMap<i64, AHashMap<u64, u32>>,  // price → (order_id → size)
    asks: BTreeMap<i64, AHashMap<u64, u32>>,
    orders: AHashMap<u64, Order>,              // order_id → Order
    best_bid: Option<i64>,                     // Cached
    best_ask: Option<i64>,
    stats: LobStats,
    last_valid_state: Option<LobState>,        // For UseLastValid policy
}
```

**Data Structure Rationale**:
- `BTreeMap<i64, ...>`: Keeps prices sorted (O(log n) insert, O(1) min/max)
- `AHashMap`: Fast hash map for order lookups (O(1) average)
- Cached `best_bid`/`best_ask`: Avoid recomputation on every message

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
BTreeMap<price, HashMap<order_id, size>>

Example bid side:
  $100.00 → { order_1001: 50, order_1002: 100 }  → aggregated: 150
  $99.99  → { order_1003: 200 }                  → aggregated: 200
```

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
    UseLastValid, // Return last known valid state
    Error,        // Return Err(TlobError::CrossedQuote)
    SkipUpdate,   // Don't update book, return last valid
}
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

---

## 7. Error Handling

### TlobError Variants (src/error.rs)

```rust
pub enum TlobError {
    InvalidOrderId(u64),       // order_id == 0
    OrderNotFound(u64),        // Operation on missing order
    InvalidPrice(i64),         // price <= 0
    InvalidSize(u32),          // size == 0
    InvalidAction(u8),         // Unknown action byte
    InvalidSide(u8),           // Unknown side byte
    SymbolNotFound(String),    // Multi-symbol: unknown symbol
    InconsistentState(String), // Generic state error
    CrossedQuote(i64, i64),    // bid >= ask (if Error policy)
    LockedQuote(i64, i64),     // bid == ask (if Error policy)
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
    // Warning counters
    pub cancel_order_not_found: u64,
    pub cancel_price_level_missing: u64,
    pub cancel_order_at_level_missing: u64,
    pub trade_order_not_found: u64,
    pub trade_price_level_missing: u64,
    pub trade_order_at_level_missing: u64,
    pub book_clears: u64,
    pub noop_messages: u64,
}
```

### DayStats (src/statistics.rs)

Aggregates LOB state statistics over a trading day:

```rust
let mut day_stats = DayStats::new("2025-02-03");
for msg in loader.iter_messages()? {
    let state = lob.process_message(&msg)?;
    day_stats.update(&state);
}
// Access: day_stats.mid_price.mean, day_stats.spread_bps.std(), etc.
```

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
    MboMessage::new(order_id, action, side, (price_dollars * 1e9) as i64, size)
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
        .skip_invalid(true);

    let mut lob = LobReconstructor::new(10);
    let mut processed = 0u64;

    for msg in loader.iter_messages().expect("Failed to iterate") {
        if let Ok(_state) = lob.process_message(&msg) {
            processed += 1;
        }
    }

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
3. **`BTreeMap`**: O(1) best price access
4. **Cached best prices**: Avoid recomputation
5. **Pre-allocated vectors**: `LobState::new(levels)` pre-sizes

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
- `LobState` for 10 levels: ~400 bytes

---

## 11. Common Patterns and Idioms

### Pattern: Processing a Day of Data

```rust
let loader = DbnLoader::new(path)?.skip_invalid(true);
let mut lob = LobReconstructor::new(10);
let mut day_stats = DayStats::new(date);

for msg in loader.iter_messages()? {
    let state = lob.process_message(&msg)?;
    day_stats.update(&state);
}

// End of day
let norm_params = NormalizationParams::from_day_stats(&day_stats, 10);
norm_params.save_json("normalization.json")?;
```

### Pattern: Multi-Day Processing

```rust
for day_file in day_files {
    lob.full_reset();  // Clear state AND stats
    day_stats = DayStats::new(extract_date(&day_file));

    for msg in DbnLoader::new(&day_file)?.iter_messages()? {
        let state = lob.process_message(&msg)?;
        day_stats.update(&state);
    }

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

### Typical Usage Pattern

```rust
use mbo_lob_reconstructor::{DbnLoader, LobReconstructor};
use feature_extractor_mbo_lob::prelude::*;

let loader = DbnLoader::new(path)?.skip_invalid(true);
let mut lob = LobReconstructor::new(20);  // Match feature extractor levels
let mut pipeline = PipelineBuilder::new()
    .with_levels(20)
    .with_preset(Preset::DeepLOB)
    .build()?;

for msg in loader.iter_messages()? {
    let state = lob.process_message(&msg)?;
    if state.is_valid() {
        // Convert LobState to feature extractor's LobSnapshot
        let snapshot = convert_state_to_snapshot(&state, msg.timestamp);
        pipeline.process(snapshot)?;
    }
}

let sequences = pipeline.finalize()?;
```

### State Conversion Helper

```rust
fn convert_state_to_snapshot(state: &LobState, timestamp: Option<i64>) -> LobSnapshot {
    // Feature extractor expects same format
    LobSnapshot {
        bid_prices: state.bid_prices.clone(),
        bid_sizes: state.bid_sizes.iter().map(|&s| s as u64).collect(),
        ask_prices: state.ask_prices.clone(),
        ask_sizes: state.ask_sizes.iter().map(|&s| s as u64).collect(),
        timestamp: timestamp.unwrap_or(0) as u64,
        levels: state.levels,
    }
}
```

---

## 13. Known Limitations and Edge Cases

### Limitations

| Limitation | Description |
|------------|-------------|
| Single-threaded | `LobReconstructor` is not thread-safe |
| No persistence | State is in-memory only |
| Fixed precision | Prices are i64 fixed-point (9 decimal places) |
| No order priority | Orders at same price have no queue priority |

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

---

## Quick Reference

### Imports for Common Tasks

```rust
// Basic reconstruction
use mbo_lob_reconstructor::{LobReconstructor, MboMessage, Action, Side};

// With configuration
use mbo_lob_reconstructor::{LobReconstructor, LobConfig, CrossedQuotePolicy};

// File loading
use mbo_lob_reconstructor::{DbnLoader, is_valid_order};

// Statistics
use mbo_lob_reconstructor::{DayStats, RunningStats, NormalizationParams};

// Analytics
use mbo_lob_reconstructor::{DepthStats, MarketImpact, LiquidityMetrics};

// Warnings
use mbo_lob_reconstructor::{WarningTracker, WarningCategory, Warning};
```

### Price Conversion

```rust
// Dollars to fixed-point
let price_fixed: i64 = (price_dollars * 1e9) as i64;

// Fixed-point to dollars
let price_dollars: f64 = price_fixed as f64 / 1e9;
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

*Last updated: 2025-02-03*
*Crate version: 0.1.0*

