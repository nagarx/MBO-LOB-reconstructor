# LOB Reconstructor Warnings & Issues Reference

This document catalogs known warnings, issues, and edge cases that may occur during LOB reconstruction. Use this as a reference when debugging preprocessing issues or investigating anomalies in live environments.

## Warning Categories

### 1. ORDER_NOT_FOUND

**Severity**: Low (1)

**Description**: A cancel or trade message references an order that doesn't exist in the book.

**Common Causes**:
- Order was already cancelled by a previous message
- Late/out-of-order message delivery
- Order existed before the data feed started (snapshot missing)
- Duplicate cancel messages

**Impact**: None - the operation is skipped safely.

**Tracked In**: `LobStats.cancel_order_not_found`, `LobStats.trade_order_not_found`

---

### 2. PRICE_LEVEL_NOT_FOUND

**Severity**: Medium (2)

**Description**: An order exists in tracking but its price level doesn't exist in the book.

**Common Causes**:
- Data inconsistency between order tracking and price levels
- Race condition in message processing
- Corrupted or incomplete data

**Impact**: Low - the orphaned order is cleaned up automatically.

**Tracked In**: `LobStats.cancel_price_level_missing`, `LobStats.trade_price_level_missing`

---

### 3. ORDER_AT_LEVEL_MISSING

**Severity**: Medium (2)

**Description**: Price level exists but the specific order is not found at that level.

**Common Causes**:
- Order was moved to a different price (modify without proper tracking)
- Data feed issue
- Internal state corruption

**Impact**: Low - the orphaned order is cleaned up automatically.

**Tracked In**: `LobStats.cancel_order_at_level_missing`, `LobStats.trade_order_at_level_missing`

---

### 4. BOOK_CLEARED

**Severity**: Low (1)

**Description**: An `Action::Clear` message was received, resetting the entire order book.

**Common Causes**:
- Market session transition (pre-market → regular → after-hours)
- Trading halt/resume
- Exchange system reset
- End of trading day

**Impact**: Normal operation - book is reset and continues processing.

**Tracked In**: `LobStats.book_clears`

---

### 5. CROSSED_QUOTES

**Severity**: High (3)

**Description**: Best bid price is greater than or equal to best ask price.

**Common Causes**:
- Aggressive order placement
- Market maker repositioning
- Data timing issues
- Locked market (bid == ask)

**Impact**: Depends on policy:
- `CrossedQuotePolicy::Allow` - Accepts crossed state
- `CrossedQuotePolicy::UseLastValid` - Returns last valid state
- `CrossedQuotePolicy::Error` - Returns error

**Tracked In**: `LobStats.crossed_quotes`, `LobStats.locked_quotes`

---

### 6. NOOP_MESSAGES

**Severity**: Low (1)

**Description**: `Action::None` messages received (no-op).

**Common Causes**:
- Heartbeat messages
- Flag-only messages
- Placeholder messages from exchange

**Impact**: None - message is ignored.

**Tracked In**: `LobStats.noop_messages`

---

## Known Edge Cases

### Pre-Market Session Start ($4.97 Error Pattern)

**Observation**: At market session boundaries (especially 04:00 AM ET), large price discrepancies (~$4.97) may appear briefly.

**Root Cause**: The MBO data stream may start mid-session without a complete book snapshot. The reconstructor builds state incrementally, so early snapshots may be incomplete.

**Mitigation**: 
- Wait for book to stabilize before using data
- Use `Action::Clear` messages as session boundary markers
- Consider skipping first N messages after session start

**Status**: Expected behavior, not a bug.

---

### Partial Cancel Handling

**Fixed in**: v0.1.0

**Previous Issue**: Partial cancellations were treated as full cancellations, removing orders entirely instead of reducing their size.

**Current Behavior**: Partial cancels correctly reduce order size. If `msg.size >= order_size`, the order is fully removed.

---

### Size Estimation Variance

**Observation**: Size accuracy is lower than price accuracy (~91% exact match vs ~99% for prices).

**Root Cause**: Size aggregation timing differences between MBO reconstruction and MBP-10 snapshots.

**Impact**: Minor - sizes are generally within acceptable range for ML training.

---

## Exporting Warnings

### Using LobStats

```rust
use mbo_lob_reconstructor::LobReconstructor;

let mut lob = LobReconstructor::new(10);
// ... process messages ...

// Check for warnings
if lob.stats().has_warnings() {
    println!("Total warnings: {}", lob.stats().total_warnings());
}

// Export to file
lob.stats().export_to_file("preprocessing_warnings.json")?;
```

### Using WarningTracker (Advanced)

```rust
use mbo_lob_reconstructor::warnings::{WarningTracker, WarningCategory};

let mut tracker = WarningTracker::new();

// Record custom warnings
tracker.record_order_warning(
    WarningCategory::OrderNotFound,
    "Order 12345 not found during cancel",
    12345,
    Some(100_000_000_000), // price
    Some(1234567890_000_000_000), // timestamp
);

// Export to JSON
tracker.export_to_file("detailed_warnings.json")?;

// Export to CSV for spreadsheet analysis
tracker.export_to_csv("warnings.csv")?;

// Get summary
let summary = tracker.summary();
println!("Total: {}, Unique orders: {}", summary.total, summary.unique_orders);
```

---

## Validation Results Summary

Based on July 2025 NVIDIA data (MBO vs MBP-10):

| Metric | Value |
|--------|-------|
| BBO Price Match | 99.17% |
| BBO Size Match | 91.15% |
| Price Within 1¢ | 99.71% |
| Mean Absolute Error | $0.000095 |
| Regular Hours Accuracy | 99.69% |

### By Time of Day

| Session | Price Accuracy |
|---------|---------------|
| Pre-market (04:00-09:30) | ~95-99% |
| Regular hours (09:30-16:00) | ~99.7% |
| After-hours (16:00-20:00) | ~99.2% |
| Extended (20:00+) | ~95-97% |

---

## Debugging Checklist

1. **Check warning counts**: `lob.stats().total_warnings()`
2. **Check for book clears**: `lob.stats().book_clears`
3. **Check crossed quotes**: `lob.stats().crossed_quotes`
4. **Export detailed stats**: `lob.stats().export_to_file("debug.json")`
5. **Enable logging**: Use `LobConfig::new(10).with_logging(true)`
6. **Check timestamps**: Ensure data is in chronological order

---

## Contact

For issues not covered here, please:
1. Check the test suite: `cargo test -p mbo-lob-reconstructor`
2. Run validation: `cargo test --test granular_mbo_mbp_validation --release --features databento`
3. Open an issue with:
   - Data sample (anonymized if needed)
   - Warning counts from `LobStats`
   - Expected vs actual behavior

