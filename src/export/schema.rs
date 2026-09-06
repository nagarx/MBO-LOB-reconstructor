//! Arrow schema definitions for LOB snapshot and MBO event Parquet files.
//!
//! These schemas are the **single source of truth** for the data contract
//! between the exporter and any downstream consumer (e.g., `raw-lob-analyzer`).
//!
//! # Column conventions
//!
//! - Prices: `Int64` in nanodollars (divide by 1e9 for dollars)
//! - Sizes: `UInt32` in shares
//! - Timestamps: `Int64` nanoseconds since epoch
//! - Enums: `UInt8` byte representation (see [`crate::Action::to_byte`], [`crate::Side::to_byte`])
//! - Arrays: `FixedSizeList` for price/size level data

use std::collections::HashMap;
use std::sync::Arc;

use arrow::datatypes::{DataType, Field, Schema};

use super::SCHEMA_VERSION;

/// Build the Arrow schema for LOB snapshot Parquet files.
///
/// # Arguments
/// - `levels`: number of LOB levels per side (determines FixedSizeList width)
/// - `include_derived`: whether to include computed analytics columns
///
/// # Column layout (core)
///
/// | Column             | Type                       | Nullable |
/// |--------------------|----------------------------|----------|
/// | timestamp_ns       | Int64                      | true     |
/// | sequence           | UInt64                     | false    | (= `message_index`, NOT the vendor's sequence)
/// | levels             | UInt8                      | false    |
/// | best_bid           | Int64                      | true     |
/// | best_ask           | Int64                      | true     |
/// | bid_prices         | FixedSizeList(Int64, N)    | false    |
/// | bid_sizes          | FixedSizeList(UInt32, N)   | false    |
/// | ask_prices         | FixedSizeList(Int64, N)    | false    |
/// | ask_sizes          | FixedSizeList(UInt32, N)   | false    |
/// | delta_ns           | UInt64                     | false    |
/// | triggering_action  | UInt8                      | true     |
/// | triggering_side    | UInt8                      | true     |
///
/// # Per-file invariant: `levels` column (Phase O Cycle 1 / B.1)
///
/// The `levels` column is a **per-file CONSTANT** equal to the `levels`
/// argument passed to this function (= `ExportConfig.levels`). It does
/// NOT vary row-to-row. For every row, `levels[i] == N == FixedSizeList
/// width`. Downstream consumers MAY rely on this invariant; e.g.,
/// `MBO-LOB-analyzer/src/rawlobanalyzer/io/schema.py:80` hardcodes
/// `pa.list_(pa.int64(), 10)`, which is correct iff every file in the
/// corpus is exported with `ExportConfig.levels == 10`. Files exported
/// with a different `levels` value form a DIFFERENT per-file schema
/// (the `levels` column value will differ).
///
/// This invariant is enforced producer-side at `LobBatch::push`
/// (`batch.rs::LobBatch::push`) via `debug_assert_eq!(state.levels,
/// self.levels)` — caller must construct `LobReconstructor` and
/// `LobSnapshotWriter` with identical `levels` so that the per-row
/// `state.levels` matches the per-file `self.levels`.
///
/// # Derived columns (when `include_derived = true`)
///
/// | Column             | Type    | Nullable |
/// |--------------------|---------|----------|
/// | mid_price          | Float64 | true     |
/// | spread             | Float64 | true     |
/// | spread_bps         | Float64 | true     |
/// | microprice         | Float64 | true     |
/// | total_bid_volume   | UInt64  | false    |
/// | total_ask_volume   | UInt64  | false    |
/// | depth_imbalance    | Float64 | true     |
/// | book_consistency   | UInt8   | false    |
pub fn lob_snapshot_schema(levels: usize, include_derived: bool) -> Schema {
    let n = levels as i32;

    let mut fields = vec![
        // NULLABLE, and deliberately so — `LobState::timestamp` is `Option<i64>`
        // and an absent venue clock must stay absent. Pre-W23 this was
        // non-nullable and `LobBatch::push` coerced `None` to `0`, so "no clock"
        // and "1970-01-01T00:00:00Z" were the same 8 bytes; the sibling
        // `mbo_event_schema` below already declared the same concept nullable.
        Field::new("timestamp_ns", DataType::Int64, true),
        // ⚠ THIS COLUMN IS *NOT* THE VENDOR'S `sequence`. It carries
        // `LobState::message_index` — this crate's own 1-based message counter
        // — so it is dense and strictly +1 per row, and a venue-gap or
        // message-loss check computed over it reports "no gaps" for ANY input,
        // forever. The vendor's own `sequence` is present on `dbn::MboMsg` and
        // is dropped at `DbnBridge::convert`; carrying it instead would change
        // this column's VALUES and is an open decision, not a rename.
        //
        // The NAME was deliberately left behind when the source field was
        // renamed on 2026-09-06 — see `super::batch::LobBatch::push` for the
        // named consumer that validates on it.
        Field::new("sequence", DataType::UInt64, false),
        Field::new("levels", DataType::UInt8, false),
        Field::new("best_bid", DataType::Int64, true),
        Field::new("best_ask", DataType::Int64, true),
        Field::new(
            "bid_prices",
            DataType::FixedSizeList(Arc::new(Field::new("item", DataType::Int64, false)), n),
            false,
        ),
        Field::new(
            "bid_sizes",
            DataType::FixedSizeList(Arc::new(Field::new("item", DataType::UInt32, false)), n),
            false,
        ),
        Field::new(
            "ask_prices",
            DataType::FixedSizeList(Arc::new(Field::new("item", DataType::Int64, false)), n),
            false,
        ),
        Field::new(
            "ask_sizes",
            DataType::FixedSizeList(Arc::new(Field::new("item", DataType::UInt32, false)), n),
            false,
        ),
        Field::new("delta_ns", DataType::UInt64, false),
        Field::new("triggering_action", DataType::UInt8, true),
        Field::new("triggering_side", DataType::UInt8, true),
    ];

    if include_derived {
        fields.extend([
            Field::new("mid_price", DataType::Float64, true),
            Field::new("spread", DataType::Float64, true),
            Field::new("spread_bps", DataType::Float64, true),
            Field::new("microprice", DataType::Float64, true),
            Field::new("total_bid_volume", DataType::UInt64, false),
            Field::new("total_ask_volume", DataType::UInt64, false),
            Field::new("depth_imbalance", DataType::Float64, true),
            Field::new("book_consistency", DataType::UInt8, false),
        ]);
    }

    let mut metadata = schema_metadata();
    // W24b — SCOPED TO THIS SCHEMA. A book level with no resting liquidity is
    // emitted as price 0 / size 0 into the non-nullable `{bid,ask}_{prices,sizes}`
    // columns, so "no level here" and "a level at $0.00" are the same bytes. The
    // encoding is not changed — re-encoding absent levels as nulls would move the
    // data under 48 downstream analysis modules — it is DECLARED, so a consumer
    // can mask on it instead of inferring it.
    //
    // ⚠ This key is meaningless for `mbo_event_schema`, which has no level
    // columns, which is why it is inserted here and not in `schema_metadata`.
    metadata.insert("absent_level_encoding".into(), "price_0_size_0".into());
    Schema::new_with_metadata(fields, metadata)
}

/// Build the Arrow schema for MBO event Parquet files.
///
/// # Column layout
///
/// | Column       | Type   | Nullable |
/// |--------------|--------|----------|
/// | timestamp_ns | Int64  | true     |
/// | order_id     | UInt64 | false    |
/// | action       | UInt8  | false    |
/// | side         | UInt8  | false    |
/// | price        | Int64  | false    |
/// | size         | UInt32 | false    |
pub fn mbo_event_schema() -> Schema {
    let fields = vec![
        Field::new("timestamp_ns", DataType::Int64, true),
        Field::new("order_id", DataType::UInt64, false),
        Field::new("action", DataType::UInt8, false),
        Field::new("side", DataType::UInt8, false),
        Field::new("price", DataType::Int64, false),
        Field::new("size", DataType::UInt32, false),
    ];

    let mut metadata = schema_metadata();
    // W24a — SCOPED TO THIS SCHEMA'S `price` FIELD. `DbnBridge::convert` fail-closes
    // `dbn::UNDEF_PRICE` on every order-bearing action and deliberately EXEMPTS
    // `Clear`/`None` (a book reset names no level), so the sentinel reaches this
    // column BY DESIGN and preserving it is correct. What was missing is the
    // declaration: the column is a non-nullable `Int64` whose only descriptor was
    // `price_unit=nanodollars`, and `i64::MAX * 1e-9` is a finite, `isfinite`-True
    // $9.22bn that silently poisons any mean taken over it.
    //
    // ⚠ NOT IN `schema_metadata` — a sentinel is a per-FIELD property of the wire
    // format (hft-rules §2), not a per-file constant. The LOB snapshot price
    // columns were measured at ZERO occurrences over 27,790,852 emitted rows;
    // declaring it there would be a false declaration on four columns.
    //
    // Spelled as a literal for the same reason `MboMessage::validate` does: this
    // module compiles without the `databento` feature and cannot name
    // `dbn::UNDEF_PRICE`. `the_mbo_price_column_declares_the_undefined_price_sentinel_it_carries`
    // in `tests/export_test.rs` is the check that the two agree.
    metadata.insert("price_undef_sentinel".into(), "9223372036854775807".into());
    Schema::new_with_metadata(fields, metadata)
}

/// Common metadata embedded in every exported Parquet file.
///
/// Per RULE.md Section 1.44/1.45: metadata propagates units and provenance
/// so downstream consumers can validate compatibility.
fn schema_metadata() -> HashMap<String, String> {
    let mut m = HashMap::new();
    m.insert("schema_version".into(), SCHEMA_VERSION.into());
    m.insert("source".into(), "mbo-lob-reconstructor".into());
    m.insert(
        "reconstructor_version".into(),
        env!("CARGO_PKG_VERSION").into(),
    );
    m.insert("price_unit".into(), "nanodollars".into());
    m.insert("size_unit".into(), "shares".into());
    m.insert("timestamp_unit".into(), "nanoseconds_since_epoch".into());
    // The `T`/`F` carrier split (L-DECODE). `"true"` means the `action` /
    // `triggering_action` columns distinguish the vendor's aggressor-side trade
    // print (`84`, `b'T'`) from the resting-order fill (`70`, `b'F'`); pre-split
    // files emitted every execution as `84` and carry NO such key.
    //
    // ⚠ THIS KEY EXISTS TO BE READ. A consumer masking `action == 84` selects ~45%
    // fewer rows against a split file than against a pre-split one, with no error
    // raised. Branch on THIS — never on "does byte 70 appear in the data?", which is
    // silent on any file that happens to contain zero fills, making absence
    // indistinguishable from agreement (hft-rules §1).
    m.insert("action_carrier_split".into(), "true".into());
    m
}

/// Number of core (non-derived) columns in the LOB snapshot schema.
pub const LOB_CORE_COLUMN_COUNT: usize = 12;

/// Number of derived columns added when `include_derived = true`.
pub const LOB_DERIVED_COLUMN_COUNT: usize = 8;

/// Number of columns in the MBO event schema.
pub const MBO_COLUMN_COUNT: usize = 6;

/// Encode a [`BookConsistency`](crate::types::BookConsistency) variant as `u8`.
///
/// | Value | Meaning |
/// |-------|---------|
/// | 0     | Valid   |
/// | 1     | Empty   |
/// | 2     | Locked  |
/// | 3     | Crossed |
pub fn book_consistency_to_u8(bc: crate::types::BookConsistency) -> u8 {
    use crate::types::BookConsistency;
    match bc {
        BookConsistency::Valid => 0,
        BookConsistency::Empty => 1,
        BookConsistency::Locked => 2,
        BookConsistency::Crossed => 3,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lob_schema_field_count_no_derived() {
        let schema = lob_snapshot_schema(10, false);
        assert_eq!(
            schema.fields().len(),
            LOB_CORE_COLUMN_COUNT,
            "Core-only LOB schema should have {LOB_CORE_COLUMN_COUNT} fields"
        );
    }

    #[test]
    fn test_lob_schema_field_count_with_derived() {
        let schema = lob_snapshot_schema(10, true);
        assert_eq!(
            schema.fields().len(),
            LOB_CORE_COLUMN_COUNT + LOB_DERIVED_COLUMN_COUNT,
            "Full LOB schema should have {} fields",
            LOB_CORE_COLUMN_COUNT + LOB_DERIVED_COLUMN_COUNT
        );
    }

    #[test]
    fn test_mbo_schema_field_count() {
        let schema = mbo_event_schema();
        assert_eq!(
            schema.fields().len(),
            MBO_COLUMN_COUNT,
            "MBO schema should have {MBO_COLUMN_COUNT} fields"
        );
    }

    #[test]
    fn test_schema_metadata_present() {
        let schema = lob_snapshot_schema(10, true);
        let meta = schema.metadata();
        assert_eq!(meta.get("schema_version").unwrap(), SCHEMA_VERSION);
        assert_eq!(meta.get("source").unwrap(), "mbo-lob-reconstructor");
        assert_eq!(meta.get("price_unit").unwrap(), "nanodollars");
        assert_eq!(meta.get("size_unit").unwrap(), "shares");
        assert_eq!(
            meta.get("timestamp_unit").unwrap(),
            "nanoseconds_since_epoch"
        );
    }

    #[test]
    fn test_lob_schema_various_levels() {
        for levels in [1, 5, 10, 20] {
            let schema = lob_snapshot_schema(levels, false);
            let bid_prices_field = schema.field_with_name("bid_prices").unwrap();
            match bid_prices_field.data_type() {
                DataType::FixedSizeList(_, n) => {
                    assert_eq!(*n, levels as i32, "FixedSizeList width should match levels");
                }
                other => panic!("Expected FixedSizeList, got {other:?}"),
            }
        }
    }

    #[test]
    fn test_lob_schema_column_names_core() {
        let schema = lob_snapshot_schema(10, false);
        let names: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();
        assert_eq!(
            names,
            vec![
                "timestamp_ns",
                "sequence",
                "levels",
                "best_bid",
                "best_ask",
                "bid_prices",
                "bid_sizes",
                "ask_prices",
                "ask_sizes",
                "delta_ns",
                "triggering_action",
                "triggering_side",
            ]
        );
    }

    #[test]
    fn test_lob_schema_column_names_derived() {
        let schema = lob_snapshot_schema(10, true);
        let names: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();
        let derived_start = LOB_CORE_COLUMN_COUNT;
        assert_eq!(
            &names[derived_start..],
            &[
                "mid_price",
                "spread",
                "spread_bps",
                "microprice",
                "total_bid_volume",
                "total_ask_volume",
                "depth_imbalance",
                "book_consistency",
            ]
        );
    }

    #[test]
    fn test_mbo_schema_column_names() {
        let schema = mbo_event_schema();
        let names: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();
        assert_eq!(
            names,
            vec![
                "timestamp_ns",
                "order_id",
                "action",
                "side",
                "price",
                "size"
            ]
        );
    }

    #[test]
    fn test_book_consistency_encoding() {
        use crate::types::BookConsistency;
        assert_eq!(book_consistency_to_u8(BookConsistency::Valid), 0);
        assert_eq!(book_consistency_to_u8(BookConsistency::Empty), 1);
        assert_eq!(book_consistency_to_u8(BookConsistency::Locked), 2);
        assert_eq!(book_consistency_to_u8(BookConsistency::Crossed), 3);
    }

    #[test]
    fn test_lob_schema_nullability() {
        let schema = lob_snapshot_schema(10, true);
        let nullable_fields = [
            // W23: an absent venue clock stays absent rather than becoming epoch 0.
            "timestamp_ns",
            "best_bid",
            "best_ask",
            "triggering_action",
            "triggering_side",
            "mid_price",
            "spread",
            "spread_bps",
            "microprice",
            "depth_imbalance",
        ];
        let non_nullable_fields = [
            "sequence",
            "levels",
            "bid_prices",
            "bid_sizes",
            "ask_prices",
            "ask_sizes",
            "delta_ns",
            "total_bid_volume",
            "total_ask_volume",
            "book_consistency",
        ];

        for name in nullable_fields {
            let field = schema.field_with_name(name).unwrap();
            assert!(field.is_nullable(), "{name} should be nullable");
        }
        for name in non_nullable_fields {
            let field = schema.field_with_name(name).unwrap();
            assert!(!field.is_nullable(), "{name} should NOT be nullable");
        }
    }
}
