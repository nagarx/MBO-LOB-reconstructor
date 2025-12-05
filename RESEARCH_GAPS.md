# Research Gap Analysis: MBO-LOB-Reconstructor

> **Purpose**: Technical analysis of missing components required to support state-of-the-art LOB deep learning models based on 22 research papers.
>
> **Scope**: Focus on what the reconstructor must provide to enable downstream feature extraction and model training.

---

## Table of Contents

1. [Executive Summary](#executive-summary)
2. [Current Implementation Status](#current-implementation-status)
3. [High Priority Gaps](#high-priority-gaps)
4. [Medium Priority Gaps](#medium-priority-gaps)
5. [Research Paper Requirements Matrix](#research-paper-requirements-matrix)
6. [Implementation Recommendations](#implementation-recommendations)

---

## Executive Summary

The MBO-LOB-reconstructor library provides the foundational data layer for the entire pipeline. Based on analysis of research papers including DeepLOB, TLOB, FI-2010, HLOB, and the MBO Paper (Zhang et al.), the reconstructor is **~85% complete** for supporting state-of-the-art models.

### Critical Gaps

| Gap | Impact | Effort |
|-----|--------|--------|
| Order Lifecycle Tracking | High | Medium |
| Day Boundary Handling | High | Low |
| Message Timestamps Preservation | High | Low |
| Trade/Execution Aggregation | Medium | Medium |

---

## Current Implementation Status

### ✅ Fully Implemented

| Component | Research Reference | Notes |
|-----------|-------------------|-------|
| BTreeMap-based LOB | Standard | O(log n) operations |
| 10-level depth support | FI-2010, DeepLOB | Configurable |
| Bid/Ask price-volume | All papers | Core feature |
| Order matching (FIFO) | Market microstructure | Standard |
| System message filtering | MBO Paper | `skip_system_messages` |
| Statistics tracking | Analytics | `LobStats` |
| Best bid/ask tracking | All papers | `best_bid`, `best_ask` |
| Mid-price calculation | All papers | Via `LobState` |

### ⚠️ Partially Implemented

| Component | Current State | Gap |
|-----------|--------------|-----|
| Order tracking | HashMap by ID | Missing lifecycle events |
| Timestamp handling | Per-message | No day boundary detection |
| Trade detection | Via Action::Fill | Missing aggregation |

---

## High Priority Gaps

### 1. Order Lifecycle Tracking

**Research Reference**: Zhang et al. "Deep Learning for Market by Order Data"

**Why Critical**: MBO data's unique advantage is tracking individual orders through their lifecycle:

```
Order Lifecycle:
  Add (Action::Add) → Modify (Action::Modify) → Cancel/Execute (Action::Cancel/Fill)
```

**Current Gap**: The reconstructor tracks orders by ID but doesn't expose:
- Order age (time since placement)
- Modification count per order
- Queue position changes over time
- Order-level statistics

**Required Data Structure**:

```rust
/// Order lifecycle information for MBO feature extraction
pub struct OrderLifecycle {
    /// Original order details
    pub order_id: u64,
    pub side: Side,
    pub original_price: i64,
    pub original_size: u64,
    pub creation_timestamp: u64,
    
    /// Lifecycle tracking
    pub modifications: Vec<OrderModification>,
    pub current_queue_position: Option<usize>,
    pub time_in_queue_ns: u64,
    
    /// Terminal state
    pub terminal_action: Option<Action>,  // Cancel, Fill, or None if still active
    pub terminal_timestamp: Option<u64>,
}

pub struct OrderModification {
    pub timestamp: u64,
    pub old_price: i64,
    pub new_price: i64,
    pub old_size: u64,
    pub new_size: u64,
}
```

**API Addition**:

```rust
impl LobReconstructor {
    /// Get lifecycle information for an order
    pub fn get_order_lifecycle(&self, order_id: u64) -> Option<&OrderLifecycle>;
    
    /// Get all active orders with lifecycle info
    pub fn active_orders_with_lifecycle(&self) -> impl Iterator<Item = &OrderLifecycle>;
    
    /// Get recently completed orders (for MBO feature extraction)
    pub fn recent_completed_orders(&self, window_ns: u64) -> Vec<&OrderLifecycle>;
}
```

**Impact**: Enables 12+ MBO features from the research paper:
- Order modification frequency
- Time-to-cancellation distribution
- Queue position volatility
- Institutional order detection (large orders with many modifications)

---

### 2. Day Boundary Detection and Handling

**Research Reference**: FI-2010, DeepLOB, LOBCAST Benchmark

**Why Critical**: All research papers split data by trading days:
- Train: Days 1-7
- Validation: Last 20% of train (or separate days)
- Test: Days 8-10

The reconstructor must detect day boundaries for:
1. Resetting statistics per day
2. Triggering normalization recalculation
3. Proper train/test split

**Current Gap**: No day boundary detection; relies on manual `full_reset()` calls.

**Required Enhancement**:

```rust
/// Day boundary detection configuration
pub struct DayBoundaryConfig {
    /// Market open time in nanoseconds since midnight (e.g., 9:30 AM = 34_200_000_000_000)
    pub market_open_ns: u64,
    
    /// Market close time in nanoseconds since midnight
    pub market_close_ns: u64,
    
    /// Minimum gap between messages to detect new day (e.g., 4 hours)
    pub day_gap_threshold_ns: u64,
    
    /// Timezone offset from UTC in hours
    pub timezone_offset_hours: i8,
}

impl Default for DayBoundaryConfig {
    fn default() -> Self {
        Self {
            market_open_ns: 34_200_000_000_000,  // 9:30 AM
            market_close_ns: 57_600_000_000_000, // 4:00 PM
            day_gap_threshold_ns: 14_400_000_000_000, // 4 hours
            timezone_offset_hours: -5, // EST
        }
    }
}

/// Day boundary event for downstream processing
#[derive(Debug, Clone)]
pub struct DayBoundary {
    pub previous_day_end_ts: u64,
    pub new_day_start_ts: u64,
    pub trading_day_index: u32,
    pub previous_day_stats: DayStats,
}
```

**API Addition**:

```rust
impl LobReconstructor {
    /// Process message with day boundary detection
    pub fn process_message_with_day_detection(
        &mut self,
        msg: &MboMessage,
    ) -> Result<(LobState, Option<DayBoundary>)>;
    
    /// Get current trading day index (0-based)
    pub fn current_trading_day(&self) -> u32;
    
    /// Check if timestamp is within trading hours
    pub fn is_trading_hours(&self, timestamp_ns: u64) -> bool;
}
```

**Impact**:
- Enables automatic train/val/test splitting
- Triggers rolling normalization recalculation
- Filters pre-market and after-hours data

---

### 3. Message Timestamp Preservation in LobState

**Research Reference**: FI-2010 (time-sensitive features u6-u9), MBO Paper

**Why Critical**: The FI-2010 benchmark uses time derivatives:
- `dP/dt`: Price derivation over time
- `dV/dt`: Volume derivation over time
- `λ`: Event arrival intensity per type

**Current Gap**: `LobState` includes `timestamp` but downstream processors need:
- Previous timestamp (for Δt calculation)
- Message type that caused the update
- Sequence number within the day

**Required Enhancement**:

```rust
/// Extended LOB state with temporal information
#[derive(Debug, Clone)]
pub struct LobState {
    // ... existing fields ...
    
    /// Timestamp of this state update
    pub timestamp: u64,
    
    /// Timestamp of previous state (for Δt calculation)
    pub previous_timestamp: Option<u64>,
    
    /// Time delta since last update in nanoseconds
    pub delta_ns: u64,
    
    /// Action that caused this state update
    pub triggering_action: Action,
    
    /// Side affected by the triggering action
    pub triggering_side: Option<Side>,
    
    /// Sequence number within current trading day (0-indexed)
    pub day_sequence_number: u64,
    
    /// Whether this is the first state of a new trading day
    pub is_day_start: bool,
}
```

**Impact**: Enables time-sensitive FI-2010 features:
- Price/volume derivatives (40 features)
- Event intensity per type (6 features)
- Relative intensity comparison (12 features)
- Activity acceleration (6 features)

---

### 4. Trade/Execution Aggregation

**Research Reference**: MBO Paper, Price Impact Literature

**Why Critical**: The MBO paper distinguishes between:
- Limit orders (passive, add liquidity)
- Market orders (aggressive, remove liquidity)
- Trades (executions, price discovery)

Trade aggregation provides:
- Trade direction (buyer vs seller initiated)
- Trade size distribution
- Aggressor side detection

**Current Gap**: Individual fills are tracked but not aggregated into trades.

**Required Enhancement**:

```rust
/// Aggregated trade information
#[derive(Debug, Clone)]
pub struct Trade {
    /// Timestamp of the trade
    pub timestamp: u64,
    
    /// Trade price
    pub price: i64,
    
    /// Total size of the trade (may aggregate multiple fills)
    pub size: u64,
    
    /// Aggressor side (who initiated the trade)
    pub aggressor_side: Side,
    
    /// Number of resting orders hit
    pub num_fills: u32,
    
    /// Order IDs involved (aggressor first, then passive)
    pub order_ids: Vec<u64>,
    
    /// Trade direction indicator (+1 for uptick, -1 for downtick, 0 for same)
    pub tick_direction: i8,
}

/// Trade accumulator for streaming aggregation
pub struct TradeAggregator {
    /// Recent trades for feature extraction
    trades: VecDeque<Trade>,
    
    /// Maximum number of trades to retain
    max_trades: usize,
    
    /// Running statistics
    pub total_buy_volume: u64,
    pub total_sell_volume: u64,
    pub trade_count: u64,
}
```

**API Addition**:

```rust
impl LobReconstructor {
    /// Get recent trades for feature extraction
    pub fn recent_trades(&self, count: usize) -> &[Trade];
    
    /// Get trade imbalance (buy_volume - sell_volume) / total_volume
    pub fn trade_imbalance(&self) -> f64;
    
    /// Get average trade size
    pub fn avg_trade_size(&self) -> f64;
    
    /// Check if last trade was buyer-initiated
    pub fn last_trade_was_buy(&self) -> Option<bool>;
}
```

**Impact**: Enables key MBO features:
- Trade intensity
- Buy/sell imbalance
- Price impact estimation
- Aggressor detection

---

## Medium Priority Gaps

### 5. Queue Position Calculation

**Research Reference**: HLOB, Queue Imbalance Literature

**Current Gap**: Orders are stored but queue position is not calculated.

```rust
impl LobReconstructor {
    /// Get queue position for an order (1-indexed from front)
    pub fn queue_position(&self, order_id: u64) -> Option<usize>;
    
    /// Get total orders ahead of a specific order
    pub fn orders_ahead(&self, order_id: u64) -> Option<u64>;
    
    /// Get volume ahead of a specific order
    pub fn volume_ahead(&self, order_id: u64) -> Option<u64>;
}
```

### 6. Level Depth Information

**Research Reference**: HLOB (volume-level dependencies)

**Current Gap**: Price levels are available but not explicitly depth-indexed.

```rust
/// Level information for HLOB-style features
pub struct LevelInfo {
    pub level_index: usize,  // 0 = best, 1 = second best, etc.
    pub price: i64,
    pub total_volume: u64,
    pub order_count: usize,
    pub price_distance_from_mid: f64,  // In ticks
}

impl LobReconstructor {
    /// Get level information for feature extraction
    pub fn bid_levels(&self) -> Vec<LevelInfo>;
    pub fn ask_levels(&self) -> Vec<LevelInfo>;
}
```

### 7. Microsecond-Resolution Statistics

**Research Reference**: HFT literature, Realized Volatility

```rust
/// High-resolution statistics for volatility estimation
pub struct MicroStats {
    /// Realized variance (sum of squared returns)
    pub realized_variance: f64,
    
    /// Number of mid-price changes
    pub price_change_count: u64,
    
    /// Average time between mid-price changes (ns)
    pub avg_price_change_interval_ns: f64,
    
    /// Maximum price jump observed
    pub max_price_jump: i64,
}
```

---

## Research Paper Requirements Matrix

| Paper | Reconstructor Requirement | Status |
|-------|--------------------------|--------|
| **DeepLOB** | 10-level LOB snapshots | ✅ |
| | Mid-price calculation | ✅ |
| | Timestamp per snapshot | ✅ |
| **TLOB** | Same as DeepLOB | ✅ |
| | Multiple horizons support | ⚠️ (downstream) |
| **FI-2010** | 10-level LOB | ✅ |
| | Time-sensitive features (u6-u9) | ⚠️ Need delta_t |
| | Event type tracking | ⚠️ Need action in state |
| **MBO Paper** | Order ID tracking | ✅ |
| | Order lifecycle | ❌ Missing |
| | Normalized price (tick-based) | ⚠️ (downstream) |
| | Trade aggregation | ❌ Missing |
| **HLOB** | Volume level dependencies | ⚠️ Partial |
| | Level indexing | ⚠️ Need explicit API |
| **BiNCTABL** | Standard LOB | ✅ |
| **LOBCAST** | Day-based splitting | ❌ Missing |
| | Multiple stocks | ✅ (stateless) |

---

## Implementation Recommendations

### Phase 1: Critical Path (Required for Training)

1. **Enhance LobState with temporal info**
   - Add `previous_timestamp`, `delta_ns`
   - Add `triggering_action`, `triggering_side`
   - Add `day_sequence_number`
   - **Effort**: 2-3 hours
   - **Impact**: Unlocks FI-2010 time-sensitive features

2. **Add Day Boundary Detection**
   - Implement `DayBoundaryConfig`
   - Add `process_message_with_day_detection()`
   - Track `current_trading_day`
   - **Effort**: 3-4 hours
   - **Impact**: Enables proper train/test splitting

### Phase 2: MBO Feature Enhancement

3. **Implement Order Lifecycle Tracking**
   - Add `OrderLifecycle` struct
   - Track modifications in order HashMap
   - Expose via `get_order_lifecycle()`
   - **Effort**: 4-6 hours
   - **Impact**: Enables full MBO feature set

4. **Add Trade Aggregation**
   - Implement `Trade` and `TradeAggregator`
   - Detect aggressor side from message flow
   - Expose via `recent_trades()`
   - **Effort**: 4-6 hours
   - **Impact**: Enables trade-based features

### Phase 3: Advanced Analytics

5. **Queue Position Calculation**
   - Add position tracking to order storage
   - Implement `queue_position()` API
   - **Effort**: 3-4 hours

6. **Level Depth Information**
   - Implement `LevelInfo` struct
   - Add `bid_levels()`, `ask_levels()` APIs
   - **Effort**: 2-3 hours

---

## Appendix: Feature Count by Gap

| Gap | Features Enabled | Research Paper |
|-----|-----------------|----------------|
| Temporal info in LobState | 64 | FI-2010 (u6-u9) |
| Day boundary detection | N/A (infrastructure) | All |
| Order lifecycle | 12 | MBO Paper |
| Trade aggregation | 8 | MBO Paper |
| Queue position | 4 | HLOB |
| Level depth | 20 | HLOB |
| **Total New Features** | **108** | |

---

*Document Version: 1.0*
*Last Updated: December 2024*
*Based on analysis of 22 research papers*

