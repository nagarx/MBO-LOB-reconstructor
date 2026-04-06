//! Comprehensive tests for the Parquet export module.
//!
//! Tests cover: schema construction, round-trip fidelity, edge cases,
//! batching, configuration, downsampling, metadata, and numerical precision.
//!
//! These tests use synthetic data and do not require real DBN files.
//!
//! Requires the `export` feature: `cargo test --features export`

#![cfg(feature = "export")]

use std::collections::HashMap;
use std::fs;

use arrow::array::{
    Array, FixedSizeListArray, Float64Array, Int64Array, UInt32Array, UInt64Array, UInt8Array,
};
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use parquet::basic::Compression;

use mbo_lob_reconstructor::export::schema::{
    book_consistency_to_u8, lob_snapshot_schema, mbo_event_schema, LOB_CORE_COLUMN_COUNT,
    LOB_DERIVED_COLUMN_COUNT, MBO_COLUMN_COUNT,
};
use mbo_lob_reconstructor::export::{
    DownsampleConfig, DownsampleStrategy, ExportConfig, LobSnapshotWriter, MboEventWriter,
    SCHEMA_VERSION,
};
use mbo_lob_reconstructor::types::{Action, BookConsistency, LobState, MboMessage, Side};

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Create a temp directory that is cleaned up on drop.
struct TempDir(std::path::PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("lob_export_test_{name}_{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn file(&self, name: &str) -> std::path::PathBuf {
        self.0.join(name)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Build a valid LobState with standard test values.
fn make_test_state(levels: usize) -> LobState {
    let mut state = LobState::new(levels);
    state.best_bid = Some(100_000_000_000); // $100.00
    state.best_ask = Some(100_010_000_000); // $100.01
    state.bid_prices[0] = 100_000_000_000;
    state.bid_sizes[0] = 100;
    state.ask_prices[0] = 100_010_000_000;
    state.ask_sizes[0] = 150;

    if levels >= 2 {
        state.bid_prices[1] = 99_990_000_000; // $99.99
        state.bid_sizes[1] = 200;
        state.ask_prices[1] = 100_020_000_000; // $100.02
        state.ask_sizes[1] = 100;
    }

    state.timestamp = Some(1_000_000_000_000_000_000); // 1e18 ns
    state.sequence = 42;
    state.delta_ns = 1_000_000; // 1ms
    state.triggering_action = Some(Action::Add);
    state.triggering_side = Some(Side::Bid);
    state
}

/// Build a test MboMessage.
fn make_test_msg(action: Action, side: Side) -> MboMessage {
    MboMessage::new(1001, action, side, 100_000_000_000, 100)
        .with_timestamp(1_000_000_000_000_000_000)
}

fn small_config(levels: usize, include_derived: bool) -> ExportConfig {
    ExportConfig {
        levels,
        include_derived,
        include_mbo_events: true,
        batch_size: 4,
        compression: Compression::UNCOMPRESSED,
        downsample: None,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Schema tests
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_lob_schema_level_counts() {
    for levels in [1, 5, 10, 20] {
        let schema = lob_snapshot_schema(levels, false);
        assert_eq!(schema.fields().len(), LOB_CORE_COLUMN_COUNT);

        let schema_d = lob_snapshot_schema(levels, true);
        assert_eq!(
            schema_d.fields().len(),
            LOB_CORE_COLUMN_COUNT + LOB_DERIVED_COLUMN_COUNT
        );
    }
}

#[test]
fn test_mbo_schema_fields() {
    let schema = mbo_event_schema();
    assert_eq!(schema.fields().len(), MBO_COLUMN_COUNT);
}

#[test]
fn test_schema_metadata_keys() {
    let schema = lob_snapshot_schema(10, false);
    let meta = schema.metadata();
    assert_eq!(meta.get("schema_version").unwrap(), SCHEMA_VERSION);
    assert_eq!(meta.get("source").unwrap(), "mbo-lob-reconstructor");
    assert_eq!(meta.get("price_unit").unwrap(), "nanodollars");
    assert_eq!(meta.get("size_unit").unwrap(), "shares");
    assert_eq!(
        meta.get("timestamp_unit").unwrap(),
        "nanoseconds_since_epoch"
    );
    assert!(meta.get("reconstructor_version").is_some());
}

#[test]
fn test_fixed_size_list_width_matches_levels() {
    for levels in [1, 5, 10, 20] {
        let schema = lob_snapshot_schema(levels, false);
        for name in ["bid_prices", "bid_sizes", "ask_prices", "ask_sizes"] {
            let field = schema.field_with_name(name).unwrap();
            match field.data_type() {
                arrow::datatypes::DataType::FixedSizeList(_, n) => {
                    assert_eq!(*n, levels as i32, "{name} FixedSizeList width");
                }
                other => panic!("Expected FixedSizeList for {name}, got {other:?}"),
            }
        }
    }
}

#[test]
fn test_book_consistency_encoding_roundtrip() {
    assert_eq!(book_consistency_to_u8(BookConsistency::Valid), 0);
    assert_eq!(book_consistency_to_u8(BookConsistency::Empty), 1);
    assert_eq!(book_consistency_to_u8(BookConsistency::Locked), 2);
    assert_eq!(book_consistency_to_u8(BookConsistency::Crossed), 3);
}

// ─────────────────────────────────────────────────────────────────────────────
// LOB Writer: round-trip tests
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_lob_single_snapshot_roundtrip() {
    let tmp = TempDir::new("single_rt");
    let path = tmp.file("lob.parquet");
    let config = small_config(2, true);

    let state = make_test_state(2);
    let mut writer = LobSnapshotWriter::new(&path, &config, HashMap::new()).unwrap();
    writer.write_snapshot(&state).unwrap();
    let stats = writer.finish().unwrap();

    assert_eq!(stats.rows_written, 1);
    assert_eq!(stats.rows_seen, 1);
    assert!(stats.row_groups >= 1);

    let file = fs::File::open(&path).unwrap();
    let reader = ParquetRecordBatchReaderBuilder::try_new(file)
        .unwrap()
        .build()
        .unwrap();

    let batches: Vec<_> = reader.into_iter().map(|b| b.unwrap()).collect();
    assert_eq!(batches.iter().map(|b| b.num_rows()).sum::<usize>(), 1);

    let batch = &batches[0];

    // Verify core columns
    let ts = batch.column_by_name("timestamp_ns").unwrap();
    let ts_arr = ts.as_any().downcast_ref::<Int64Array>().unwrap();
    assert_eq!(ts_arr.value(0), 1_000_000_000_000_000_000);

    let seq = batch.column_by_name("sequence").unwrap();
    let seq_arr = seq.as_any().downcast_ref::<UInt64Array>().unwrap();
    assert_eq!(seq_arr.value(0), 42);

    let lvl = batch.column_by_name("levels").unwrap();
    let lvl_arr = lvl.as_any().downcast_ref::<UInt8Array>().unwrap();
    assert_eq!(lvl_arr.value(0), 2);

    // best_bid
    let bb = batch.column_by_name("best_bid").unwrap();
    let bb_arr = bb.as_any().downcast_ref::<Int64Array>().unwrap();
    assert!(!bb_arr.is_null(0));
    assert_eq!(bb_arr.value(0), 100_000_000_000);

    // best_ask
    let ba = batch.column_by_name("best_ask").unwrap();
    let ba_arr = ba.as_any().downcast_ref::<Int64Array>().unwrap();
    assert_eq!(ba_arr.value(0), 100_010_000_000);

    // delta_ns
    let dn = batch.column_by_name("delta_ns").unwrap();
    let dn_arr = dn.as_any().downcast_ref::<UInt64Array>().unwrap();
    assert_eq!(dn_arr.value(0), 1_000_000);

    // triggering_action
    let ta = batch.column_by_name("triggering_action").unwrap();
    let ta_arr = ta.as_any().downcast_ref::<UInt8Array>().unwrap();
    assert!(!ta_arr.is_null(0));
    assert_eq!(ta_arr.value(0), Action::Add.to_byte());

    // triggering_side
    let ts_col = batch.column_by_name("triggering_side").unwrap();
    let ts_arr2 = ts_col.as_any().downcast_ref::<UInt8Array>().unwrap();
    assert_eq!(ts_arr2.value(0), Side::Bid.to_byte());

    // Derived: mid_price
    let mp = batch.column_by_name("mid_price").unwrap();
    let mp_arr = mp.as_any().downcast_ref::<Float64Array>().unwrap();
    let expected_mid = state.mid_price().unwrap();
    assert!(
        (mp_arr.value(0) - expected_mid).abs() < 1e-10,
        "mid_price mismatch: got {} expected {}",
        mp_arr.value(0),
        expected_mid
    );

    // Derived: spread
    let sp = batch.column_by_name("spread").unwrap();
    let sp_arr = sp.as_any().downcast_ref::<Float64Array>().unwrap();
    let expected_spread = state.spread().unwrap();
    assert!((sp_arr.value(0) - expected_spread).abs() < 1e-10);

    // Derived: book_consistency
    let bc = batch.column_by_name("book_consistency").unwrap();
    let bc_arr = bc.as_any().downcast_ref::<UInt8Array>().unwrap();
    assert_eq!(
        bc_arr.value(0),
        book_consistency_to_u8(BookConsistency::Valid)
    );
}

#[test]
fn test_lob_fixed_size_list_values() {
    let tmp = TempDir::new("fsl_vals");
    let path = tmp.file("lob.parquet");
    let config = small_config(3, false);

    let mut state = LobState::new(3);
    state.bid_prices[0] = 100_000_000_000;
    state.bid_prices[1] = 99_000_000_000;
    state.bid_prices[2] = 98_000_000_000;
    state.bid_sizes[0] = 100;
    state.bid_sizes[1] = 200;
    state.bid_sizes[2] = 300;
    state.ask_prices[0] = 101_000_000_000;
    state.ask_prices[1] = 102_000_000_000;
    state.ask_prices[2] = 0;
    state.ask_sizes[0] = 50;
    state.ask_sizes[1] = 75;
    state.ask_sizes[2] = 0;
    state.timestamp = Some(100);
    state.best_bid = Some(100_000_000_000);
    state.best_ask = Some(101_000_000_000);

    let mut writer = LobSnapshotWriter::new(&path, &config, HashMap::new()).unwrap();
    writer.write_snapshot(&state).unwrap();
    writer.finish().unwrap();

    let file = fs::File::open(&path).unwrap();
    let reader = ParquetRecordBatchReaderBuilder::try_new(file)
        .unwrap()
        .build()
        .unwrap();
    let batch = reader.into_iter().next().unwrap().unwrap();

    let bid_prices = batch.column_by_name("bid_prices").unwrap();
    let fsl = bid_prices
        .as_any()
        .downcast_ref::<FixedSizeListArray>()
        .unwrap();
    let inner = fsl.value(0);
    let inner_arr = inner.as_any().downcast_ref::<Int64Array>().unwrap();
    assert_eq!(inner_arr.value(0), 100_000_000_000);
    assert_eq!(inner_arr.value(1), 99_000_000_000);
    assert_eq!(inner_arr.value(2), 98_000_000_000);

    let bid_sizes = batch.column_by_name("bid_sizes").unwrap();
    let fsl_s = bid_sizes
        .as_any()
        .downcast_ref::<FixedSizeListArray>()
        .unwrap();
    let inner_s = fsl_s.value(0);
    let inner_s_arr = inner_s.as_any().downcast_ref::<UInt32Array>().unwrap();
    assert_eq!(inner_s_arr.value(0), 100);
    assert_eq!(inner_s_arr.value(1), 200);
    assert_eq!(inner_s_arr.value(2), 300);

    let ask_sizes = batch.column_by_name("ask_sizes").unwrap();
    let fsl_as = ask_sizes
        .as_any()
        .downcast_ref::<FixedSizeListArray>()
        .unwrap();
    let inner_as = fsl_as.value(0);
    let inner_as_arr = inner_as.as_any().downcast_ref::<UInt32Array>().unwrap();
    assert_eq!(inner_as_arr.value(2), 0, "Unused level should be zero");
}

// ─────────────────────────────────────────────────────────────────────────────
// Edge cases
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_lob_empty_book() {
    let tmp = TempDir::new("empty_book");
    let path = tmp.file("lob.parquet");
    let config = small_config(2, true);

    let state = LobState::new(2); // empty book

    let mut writer = LobSnapshotWriter::new(&path, &config, HashMap::new()).unwrap();
    writer.write_snapshot(&state).unwrap();
    writer.finish().unwrap();

    let file = fs::File::open(&path).unwrap();
    let reader = ParquetRecordBatchReaderBuilder::try_new(file)
        .unwrap()
        .build()
        .unwrap();
    let batch = reader.into_iter().next().unwrap().unwrap();

    // best_bid should be null
    let bb = batch.column_by_name("best_bid").unwrap();
    let bb_arr = bb.as_any().downcast_ref::<Int64Array>().unwrap();
    assert!(bb_arr.is_null(0), "Empty book best_bid should be null");

    // mid_price should be null
    let mp = batch.column_by_name("mid_price").unwrap();
    let mp_arr = mp.as_any().downcast_ref::<Float64Array>().unwrap();
    assert!(mp_arr.is_null(0), "Empty book mid_price should be null");

    // book_consistency should be Empty (1)
    let bc = batch.column_by_name("book_consistency").unwrap();
    let bc_arr = bc.as_any().downcast_ref::<UInt8Array>().unwrap();
    assert_eq!(
        bc_arr.value(0),
        book_consistency_to_u8(BookConsistency::Empty)
    );
}

#[test]
fn test_lob_crossed_book() {
    let tmp = TempDir::new("crossed");
    let path = tmp.file("lob.parquet");
    let config = small_config(2, true);

    let mut state = LobState::new(2);
    state.best_bid = Some(100_010_000_000); // bid > ask -> crossed
    state.best_ask = Some(100_000_000_000);
    state.bid_prices[0] = 100_010_000_000;
    state.bid_sizes[0] = 100;
    state.ask_prices[0] = 100_000_000_000;
    state.ask_sizes[0] = 100;
    state.timestamp = Some(100);

    let mut writer = LobSnapshotWriter::new(&path, &config, HashMap::new()).unwrap();
    writer.write_snapshot(&state).unwrap();
    writer.finish().unwrap();

    let file = fs::File::open(&path).unwrap();
    let reader = ParquetRecordBatchReaderBuilder::try_new(file)
        .unwrap()
        .build()
        .unwrap();
    let batch = reader.into_iter().next().unwrap().unwrap();

    let bc = batch.column_by_name("book_consistency").unwrap();
    let bc_arr = bc.as_any().downcast_ref::<UInt8Array>().unwrap();
    assert_eq!(
        bc_arr.value(0),
        book_consistency_to_u8(BookConsistency::Crossed)
    );
}

#[test]
fn test_lob_locked_book() {
    let tmp = TempDir::new("locked");
    let path = tmp.file("lob.parquet");
    let config = small_config(2, true);

    let mut state = LobState::new(2);
    state.best_bid = Some(100_000_000_000);
    state.best_ask = Some(100_000_000_000); // bid == ask -> locked
    state.bid_prices[0] = 100_000_000_000;
    state.bid_sizes[0] = 100;
    state.ask_prices[0] = 100_000_000_000;
    state.ask_sizes[0] = 100;
    state.timestamp = Some(100);

    let mut writer = LobSnapshotWriter::new(&path, &config, HashMap::new()).unwrap();
    writer.write_snapshot(&state).unwrap();
    writer.finish().unwrap();

    let file = fs::File::open(&path).unwrap();
    let reader = ParquetRecordBatchReaderBuilder::try_new(file)
        .unwrap()
        .build()
        .unwrap();
    let batch = reader.into_iter().next().unwrap().unwrap();

    let bc = batch.column_by_name("book_consistency").unwrap();
    let bc_arr = bc.as_any().downcast_ref::<UInt8Array>().unwrap();
    assert_eq!(
        bc_arr.value(0),
        book_consistency_to_u8(BookConsistency::Locked)
    );
}

#[test]
fn test_lob_single_sided_book() {
    let tmp = TempDir::new("single_side");
    let path = tmp.file("lob.parquet");
    let config = small_config(2, true);

    let mut state = LobState::new(2);
    state.best_bid = Some(100_000_000_000);
    // ask side empty
    state.bid_prices[0] = 100_000_000_000;
    state.bid_sizes[0] = 100;
    state.timestamp = Some(100);

    let mut writer = LobSnapshotWriter::new(&path, &config, HashMap::new()).unwrap();
    writer.write_snapshot(&state).unwrap();
    writer.finish().unwrap();

    let file = fs::File::open(&path).unwrap();
    let reader = ParquetRecordBatchReaderBuilder::try_new(file)
        .unwrap()
        .build()
        .unwrap();
    let batch = reader.into_iter().next().unwrap().unwrap();

    let ba = batch.column_by_name("best_ask").unwrap();
    let ba_arr = ba.as_any().downcast_ref::<Int64Array>().unwrap();
    assert!(ba_arr.is_null(0), "No ask: best_ask should be null");

    let mp = batch.column_by_name("mid_price").unwrap();
    let mp_arr = mp.as_any().downcast_ref::<Float64Array>().unwrap();
    assert!(mp_arr.is_null(0), "No ask: mid_price should be null");
}

#[test]
fn test_lob_no_triggering_action() {
    let tmp = TempDir::new("no_trig");
    let path = tmp.file("lob.parquet");
    let config = small_config(2, false);

    let mut state = make_test_state(2);
    state.triggering_action = None;
    state.triggering_side = None;

    let mut writer = LobSnapshotWriter::new(&path, &config, HashMap::new()).unwrap();
    writer.write_snapshot(&state).unwrap();
    writer.finish().unwrap();

    let file = fs::File::open(&path).unwrap();
    let reader = ParquetRecordBatchReaderBuilder::try_new(file)
        .unwrap()
        .build()
        .unwrap();
    let batch = reader.into_iter().next().unwrap().unwrap();

    let ta = batch.column_by_name("triggering_action").unwrap();
    let ta_arr = ta.as_any().downcast_ref::<UInt8Array>().unwrap();
    assert!(ta_arr.is_null(0));

    let ts = batch.column_by_name("triggering_side").unwrap();
    let ts_arr = ts.as_any().downcast_ref::<UInt8Array>().unwrap();
    assert!(ts_arr.is_null(0));
}

// ─────────────────────────────────────────────────────────────────────────────
// Batching tests
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_lob_batch_flush_at_capacity() {
    let tmp = TempDir::new("batch_cap");
    let path = tmp.file("lob.parquet");
    let config = small_config(2, false); // batch_size = 4

    let state = make_test_state(2);
    let mut writer = LobSnapshotWriter::new(&path, &config, HashMap::new()).unwrap();

    for _ in 0..10 {
        writer.write_snapshot(&state).unwrap();
    }
    let stats = writer.finish().unwrap();

    assert_eq!(stats.rows_written, 10);
    assert_eq!(stats.rows_seen, 10);
    // 10 rows / batch_size 4 = 2 full flushes + 1 partial = 3 row groups
    assert_eq!(stats.row_groups, 3);
}

#[test]
fn test_lob_partial_batch_on_finish() {
    let tmp = TempDir::new("partial");
    let path = tmp.file("lob.parquet");
    let config = small_config(2, false); // batch_size = 4

    let state = make_test_state(2);
    let mut writer = LobSnapshotWriter::new(&path, &config, HashMap::new()).unwrap();

    for _ in 0..3 {
        writer.write_snapshot(&state).unwrap();
    }
    let stats = writer.finish().unwrap();

    assert_eq!(stats.rows_written, 3);
    assert_eq!(stats.row_groups, 1); // only partial flush on finish

    let file = fs::File::open(&path).unwrap();
    let reader = ParquetRecordBatchReaderBuilder::try_new(file)
        .unwrap()
        .build()
        .unwrap();
    let total: usize = reader.into_iter().map(|b| b.unwrap().num_rows()).sum();
    assert_eq!(total, 3);
}

#[test]
fn test_lob_exact_batch_boundary() {
    let tmp = TempDir::new("exact_bound");
    let path = tmp.file("lob.parquet");
    let config = small_config(2, false); // batch_size = 4

    let state = make_test_state(2);
    let mut writer = LobSnapshotWriter::new(&path, &config, HashMap::new()).unwrap();

    for _ in 0..8 {
        writer.write_snapshot(&state).unwrap();
    }
    let stats = writer.finish().unwrap();

    assert_eq!(stats.rows_written, 8);
    assert_eq!(stats.row_groups, 2); // exactly 2 full flushes
}

// ─────────────────────────────────────────────────────────────────────────────
// MBO Writer tests
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_mbo_all_action_variants() {
    let tmp = TempDir::new("mbo_actions");
    let path = tmp.file("mbo.parquet");
    let config = small_config(2, false);

    let actions = [
        Action::Add,
        Action::Modify,
        Action::Cancel,
        Action::Trade,
        Action::Fill,
        Action::Clear,
        Action::None,
    ];

    let mut writer = MboEventWriter::new(&path, &config, HashMap::new()).unwrap();
    for action in &actions {
        let msg = make_test_msg(*action, Side::Bid);
        writer.write_event(&msg).unwrap();
    }
    let stats = writer.finish().unwrap();

    assert_eq!(stats.rows_written, 7);

    let file = fs::File::open(&path).unwrap();
    let reader = ParquetRecordBatchReaderBuilder::try_new(file)
        .unwrap()
        .build()
        .unwrap();
    let batches: Vec<_> = reader.into_iter().map(|b| b.unwrap()).collect();
    let total: usize = batches.iter().map(|b| b.num_rows()).sum();
    assert_eq!(total, 7);

    // Verify action bytes
    let batch = &batches[0];
    let action_col = batch.column_by_name("action").unwrap();
    let action_arr = action_col.as_any().downcast_ref::<UInt8Array>().unwrap();
    for (i, action) in actions.iter().enumerate() {
        if i < batch.num_rows() {
            assert_eq!(action_arr.value(i), action.to_byte());
        }
    }
}

#[test]
fn test_mbo_all_side_variants() {
    let tmp = TempDir::new("mbo_sides");
    let path = tmp.file("mbo.parquet");
    let config = small_config(2, false);

    let sides = [Side::Bid, Side::Ask, Side::None];

    let mut writer = MboEventWriter::new(&path, &config, HashMap::new()).unwrap();
    for side in &sides {
        let msg = make_test_msg(Action::Add, *side);
        writer.write_event(&msg).unwrap();
    }
    let stats = writer.finish().unwrap();
    assert_eq!(stats.rows_written, 3);

    let file = fs::File::open(&path).unwrap();
    let reader = ParquetRecordBatchReaderBuilder::try_new(file)
        .unwrap()
        .build()
        .unwrap();
    let batch = reader.into_iter().next().unwrap().unwrap();
    let side_col = batch.column_by_name("side").unwrap();
    let side_arr = side_col.as_any().downcast_ref::<UInt8Array>().unwrap();
    for (i, side) in sides.iter().enumerate() {
        assert_eq!(side_arr.value(i), side.to_byte());
    }
}

#[test]
fn test_mbo_roundtrip_fidelity() {
    let tmp = TempDir::new("mbo_rt");
    let path = tmp.file("mbo.parquet");
    let config = small_config(2, false);

    let msg = MboMessage::new(9999, Action::Trade, Side::Ask, 123_456_789_000, 42)
        .with_timestamp(987_654_321_000_000_000);

    let mut writer = MboEventWriter::new(&path, &config, HashMap::new()).unwrap();
    writer.write_event(&msg).unwrap();
    writer.finish().unwrap();

    let file = fs::File::open(&path).unwrap();
    let reader = ParquetRecordBatchReaderBuilder::try_new(file)
        .unwrap()
        .build()
        .unwrap();
    let batch = reader.into_iter().next().unwrap().unwrap();

    let ts = batch
        .column_by_name("timestamp_ns")
        .unwrap()
        .as_any()
        .downcast_ref::<Int64Array>()
        .unwrap();
    assert_eq!(ts.value(0), 987_654_321_000_000_000);

    let oid = batch
        .column_by_name("order_id")
        .unwrap()
        .as_any()
        .downcast_ref::<UInt64Array>()
        .unwrap();
    assert_eq!(oid.value(0), 9999);

    let price = batch
        .column_by_name("price")
        .unwrap()
        .as_any()
        .downcast_ref::<Int64Array>()
        .unwrap();
    assert_eq!(price.value(0), 123_456_789_000);

    let size = batch
        .column_by_name("size")
        .unwrap()
        .as_any()
        .downcast_ref::<UInt32Array>()
        .unwrap();
    assert_eq!(size.value(0), 42);
}

#[test]
fn test_mbo_null_timestamp() {
    let tmp = TempDir::new("mbo_null_ts");
    let path = tmp.file("mbo.parquet");
    let config = small_config(2, false);

    let msg = MboMessage::new(1, Action::Add, Side::Bid, 100_000_000_000, 100);
    // timestamp is None by default

    let mut writer = MboEventWriter::new(&path, &config, HashMap::new()).unwrap();
    writer.write_event(&msg).unwrap();
    writer.finish().unwrap();

    let file = fs::File::open(&path).unwrap();
    let reader = ParquetRecordBatchReaderBuilder::try_new(file)
        .unwrap()
        .build()
        .unwrap();
    let batch = reader.into_iter().next().unwrap().unwrap();

    let ts = batch
        .column_by_name("timestamp_ns")
        .unwrap()
        .as_any()
        .downcast_ref::<Int64Array>()
        .unwrap();
    assert!(ts.is_null(0), "None timestamp should be null in Parquet");
}

// ─────────────────────────────────────────────────────────────────────────────
// Config tests
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_default_config_produces_valid_output() {
    let tmp = TempDir::new("default_cfg");
    let path = tmp.file("lob.parquet");
    let config = ExportConfig::default();

    let state = make_test_state(10);
    let mut writer = LobSnapshotWriter::new(&path, &config, HashMap::new()).unwrap();
    writer.write_snapshot(&state).unwrap();
    let stats = writer.finish().unwrap();
    assert_eq!(stats.rows_written, 1);
}

#[test]
fn test_no_derived_omits_columns() {
    let tmp = TempDir::new("no_derived");
    let path = tmp.file("lob.parquet");
    let config = small_config(2, false);

    let state = make_test_state(2);
    let mut writer = LobSnapshotWriter::new(&path, &config, HashMap::new()).unwrap();
    writer.write_snapshot(&state).unwrap();
    writer.finish().unwrap();

    let file = fs::File::open(&path).unwrap();
    let reader = ParquetRecordBatchReaderBuilder::try_new(file)
        .unwrap()
        .build()
        .unwrap();
    let batch = reader.into_iter().next().unwrap().unwrap();

    assert!(batch.column_by_name("mid_price").is_none());
    assert!(batch.column_by_name("spread").is_none());
    assert!(batch.column_by_name("book_consistency").is_none());
}

#[test]
fn test_levels_1() {
    let tmp = TempDir::new("levels1");
    let path = tmp.file("lob.parquet");
    let config = small_config(1, false);

    let mut state = LobState::new(1);
    state.bid_prices[0] = 100_000_000_000;
    state.bid_sizes[0] = 100;
    state.ask_prices[0] = 100_010_000_000;
    state.ask_sizes[0] = 50;
    state.best_bid = Some(100_000_000_000);
    state.best_ask = Some(100_010_000_000);
    state.timestamp = Some(100);

    let mut writer = LobSnapshotWriter::new(&path, &config, HashMap::new()).unwrap();
    writer.write_snapshot(&state).unwrap();
    writer.finish().unwrap();

    let file = fs::File::open(&path).unwrap();
    let reader = ParquetRecordBatchReaderBuilder::try_new(file)
        .unwrap()
        .build()
        .unwrap();
    let batch = reader.into_iter().next().unwrap().unwrap();

    let bp = batch.column_by_name("bid_prices").unwrap();
    let fsl = bp.as_any().downcast_ref::<FixedSizeListArray>().unwrap();
    assert_eq!(fsl.value_length(), 1);
}

#[test]
fn test_uncompressed_produces_valid_file() {
    let tmp = TempDir::new("uncompressed");
    let path = tmp.file("lob.parquet");
    let mut config = small_config(2, true);
    config.compression = Compression::UNCOMPRESSED;

    let state = make_test_state(2);
    let mut writer = LobSnapshotWriter::new(&path, &config, HashMap::new()).unwrap();
    writer.write_snapshot(&state).unwrap();
    let stats = writer.finish().unwrap();
    assert_eq!(stats.rows_written, 1);
}

// ─────────────────────────────────────────────────────────────────────────────
// Downsample tests
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_downsample_every_n() {
    let tmp = TempDir::new("ds_every_n");
    let path = tmp.file("lob.parquet");
    let mut config = small_config(2, false);
    config.batch_size = 128;
    config.downsample = Some(DownsampleConfig {
        strategy: DownsampleStrategy::EveryN(5),
    });

    let state = make_test_state(2);
    let mut writer = LobSnapshotWriter::new(&path, &config, HashMap::new()).unwrap();

    for _ in 0..20 {
        writer.write_snapshot(&state).unwrap();
    }
    let stats = writer.finish().unwrap();

    assert_eq!(stats.rows_seen, 20);
    // Uses 0-based indexing: (rows_seen - 1) % 5 == 0
    // Writes at rows_seen = 1, 6, 11, 16 = 4 snapshots (always includes first)
    assert_eq!(stats.rows_written, 4);
}

#[test]
fn test_downsample_min_interval() {
    let tmp = TempDir::new("ds_interval");
    let path = tmp.file("lob.parquet");
    let mut config = small_config(2, false);
    config.batch_size = 128;
    config.downsample = Some(DownsampleConfig {
        strategy: DownsampleStrategy::MinIntervalNs(1_000_000_000), // 1s
    });

    let mut writer = LobSnapshotWriter::new(&path, &config, HashMap::new()).unwrap();

    for i in 0u64..10 {
        let mut state = make_test_state(2);
        state.timestamp = Some((i * 500_000_000) as i64); // every 0.5s
        writer.write_snapshot(&state).unwrap();
    }
    let stats = writer.finish().unwrap();

    assert_eq!(stats.rows_seen, 10);
    // t=0 (written), t=0.5 (skip), t=1 (written), t=1.5 (skip), t=2 (written)...
    // = 0, 1, 2, 3, 4 seconds → 5 snapshots
    assert_eq!(stats.rows_written, 5);
}

#[test]
fn test_downsample_none_strategy() {
    let tmp = TempDir::new("ds_none");
    let path = tmp.file("lob.parquet");
    let mut config = small_config(2, false);
    config.batch_size = 128;
    config.downsample = Some(DownsampleConfig {
        strategy: DownsampleStrategy::None,
    });

    let state = make_test_state(2);
    let mut writer = LobSnapshotWriter::new(&path, &config, HashMap::new()).unwrap();
    for _ in 0..7 {
        writer.write_snapshot(&state).unwrap();
    }
    let stats = writer.finish().unwrap();

    assert_eq!(
        stats.rows_written, 7,
        "DownsampleStrategy::None should export all"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Metadata tests
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_file_metadata_contains_required_keys() {
    let tmp = TempDir::new("meta_keys");
    let path = tmp.file("lob.parquet");
    let config = small_config(5, true);

    let mut meta = HashMap::new();
    meta.insert("date".into(), "2025-02-03".into());
    meta.insert("symbol".into(), "NVDA".into());

    let state = make_test_state(5);
    let mut writer = LobSnapshotWriter::new(&path, &config, meta).unwrap();
    writer.write_snapshot(&state).unwrap();
    writer.finish().unwrap();

    let file = fs::File::open(&path).unwrap();
    let reader_builder = ParquetRecordBatchReaderBuilder::try_new(file).unwrap();
    let file_meta = reader_builder.metadata().file_metadata();
    let schema = reader_builder.schema();
    let kv = schema.metadata();

    assert_eq!(kv.get("schema_version").unwrap(), SCHEMA_VERSION);
    assert_eq!(kv.get("source").unwrap(), "mbo-lob-reconstructor");
    assert_eq!(kv.get("price_unit").unwrap(), "nanodollars");
    assert_eq!(kv.get("size_unit").unwrap(), "shares");
    assert_eq!(kv.get("timestamp_unit").unwrap(), "nanoseconds_since_epoch");
    assert_eq!(kv.get("date").unwrap(), "2025-02-03");
    assert_eq!(kv.get("symbol").unwrap(), "NVDA");
    assert_eq!(kv.get("lob_levels").unwrap(), "5");
    assert!(kv.get("reconstructor_version").is_some());
    assert!(file_meta.num_rows() > 0);
}

#[test]
fn test_mbo_metadata() {
    let tmp = TempDir::new("mbo_meta");
    let path = tmp.file("mbo.parquet");
    let config = small_config(2, false);

    let mut meta = HashMap::new();
    meta.insert("date".into(), "2025-03-15".into());
    meta.insert("symbol".into(), "AAPL".into());

    let msg = make_test_msg(Action::Add, Side::Bid);
    let mut writer = MboEventWriter::new(&path, &config, meta).unwrap();
    writer.write_event(&msg).unwrap();
    writer.finish().unwrap();

    let file = fs::File::open(&path).unwrap();
    let reader_builder = ParquetRecordBatchReaderBuilder::try_new(file).unwrap();
    let schema = reader_builder.schema();
    let kv = schema.metadata();

    assert_eq!(kv.get("schema_version").unwrap(), SCHEMA_VERSION);
    assert_eq!(kv.get("date").unwrap(), "2025-03-15");
    assert_eq!(kv.get("symbol").unwrap(), "AAPL");
}

// ─────────────────────────────────────────────────────────────────────────────
// Numerical precision tests (per RULE.md Section 2)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_price_nanodollar_roundtrip() {
    let tmp = TempDir::new("price_rt");
    let path = tmp.file("lob.parquet");
    let config = small_config(2, false);

    let mut state = LobState::new(2);
    let prices: [i64; 4] = [
        100_000_000_000, // $100.00
        100_005_000_000, // $100.005
        99_999_999_999,  // $99.999999999 (maximum precision)
        1,               // $0.000000001 (minimum representable)
    ];

    for &price in &prices {
        state.best_bid = Some(price);
        state.best_ask = Some(price + 10_000_000); // +$0.01
        state.bid_prices[0] = price;
        state.bid_sizes[0] = 1;
        state.ask_prices[0] = price + 10_000_000;
        state.ask_sizes[0] = 1;
        state.timestamp = Some(100);

        let mut writer = LobSnapshotWriter::new(&path, &config, HashMap::new()).unwrap();
        writer.write_snapshot(&state).unwrap();
        writer.finish().unwrap();

        let file = fs::File::open(&path).unwrap();
        let reader = ParquetRecordBatchReaderBuilder::try_new(file)
            .unwrap()
            .build()
            .unwrap();
        let batch = reader.into_iter().next().unwrap().unwrap();

        let bb = batch.column_by_name("best_bid").unwrap();
        let bb_arr = bb.as_any().downcast_ref::<Int64Array>().unwrap();
        assert_eq!(
            bb_arr.value(0),
            price,
            "Price {price} should round-trip exactly through Parquet i64"
        );
    }
}

#[test]
fn test_derived_values_match_lobstate_methods() {
    let tmp = TempDir::new("derived_match");
    let path = tmp.file("lob.parquet");
    let config = small_config(5, true);

    let state = make_test_state(5);
    let expected_mid = state.mid_price().unwrap();
    let expected_spread = state.spread().unwrap();
    let expected_bps = state.spread_bps().unwrap();
    let expected_micro = state.microprice().unwrap();
    let expected_bid_vol = state.total_bid_volume();
    let expected_ask_vol = state.total_ask_volume();
    let expected_imbalance = state.depth_imbalance().unwrap();

    let mut writer = LobSnapshotWriter::new(&path, &config, HashMap::new()).unwrap();
    writer.write_snapshot(&state).unwrap();
    writer.finish().unwrap();

    let file = fs::File::open(&path).unwrap();
    let reader = ParquetRecordBatchReaderBuilder::try_new(file)
        .unwrap()
        .build()
        .unwrap();
    let batch = reader.into_iter().next().unwrap().unwrap();

    let mp = batch
        .column_by_name("mid_price")
        .unwrap()
        .as_any()
        .downcast_ref::<Float64Array>()
        .unwrap();
    assert!(
        (mp.value(0) - expected_mid).abs() < 1e-10,
        "mid_price: got {} expected {expected_mid}",
        mp.value(0)
    );

    let sp = batch
        .column_by_name("spread")
        .unwrap()
        .as_any()
        .downcast_ref::<Float64Array>()
        .unwrap();
    assert!((sp.value(0) - expected_spread).abs() < 1e-10);

    let bps = batch
        .column_by_name("spread_bps")
        .unwrap()
        .as_any()
        .downcast_ref::<Float64Array>()
        .unwrap();
    assert!((bps.value(0) - expected_bps).abs() < 1e-6);

    let micro = batch
        .column_by_name("microprice")
        .unwrap()
        .as_any()
        .downcast_ref::<Float64Array>()
        .unwrap();
    assert!((micro.value(0) - expected_micro).abs() < 1e-10);

    let bv = batch
        .column_by_name("total_bid_volume")
        .unwrap()
        .as_any()
        .downcast_ref::<UInt64Array>()
        .unwrap();
    assert_eq!(bv.value(0), expected_bid_vol);

    let av = batch
        .column_by_name("total_ask_volume")
        .unwrap()
        .as_any()
        .downcast_ref::<UInt64Array>()
        .unwrap();
    assert_eq!(av.value(0), expected_ask_vol);

    let di = batch
        .column_by_name("depth_imbalance")
        .unwrap()
        .as_any()
        .downcast_ref::<Float64Array>()
        .unwrap();
    assert!((di.value(0) - expected_imbalance).abs() < 1e-10);
}

// ─────────────────────────────────────────────────────────────────────────────
// Builder API tests
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_lob_builder_api() {
    let tmp = TempDir::new("builder");
    let path = tmp.file("lob.parquet");

    let mut writer = LobSnapshotWriter::builder(&path)
        .levels(5)
        .include_derived(true)
        .batch_size(8)
        .compression(Compression::UNCOMPRESSED)
        .date("2025-02-03")
        .symbol("NVDA")
        .metadata("custom_key", "custom_value")
        .build()
        .unwrap();

    let state = make_test_state(5);
    writer.write_snapshot(&state).unwrap();
    let stats = writer.finish().unwrap();
    assert_eq!(stats.rows_written, 1);

    let file = fs::File::open(&path).unwrap();
    let reader_builder = ParquetRecordBatchReaderBuilder::try_new(file).unwrap();
    let kv = reader_builder.schema().metadata().clone();
    assert_eq!(kv.get("date").unwrap(), "2025-02-03");
    assert_eq!(kv.get("symbol").unwrap(), "NVDA");
    assert_eq!(kv.get("custom_key").unwrap(), "custom_value");
    assert_eq!(kv.get("lob_levels").unwrap(), "5");
}

#[test]
fn test_mbo_builder_api() {
    let tmp = TempDir::new("mbo_builder");
    let path = tmp.file("mbo.parquet");

    let mut writer = MboEventWriter::builder(&path)
        .batch_size(8)
        .compression(Compression::UNCOMPRESSED)
        .date("2025-02-03")
        .symbol("NVDA")
        .build()
        .unwrap();

    let msg = make_test_msg(Action::Add, Side::Bid);
    writer.write_event(&msg).unwrap();
    let stats = writer.finish().unwrap();
    assert_eq!(stats.rows_written, 1);
}

// ─────────────────────────────────────────────────────────────────────────────
// Multi-row consistency test
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_lob_multi_row_varying_states() {
    let tmp = TempDir::new("multi_row");
    let path = tmp.file("lob.parquet");
    let mut config = small_config(2, true);
    config.batch_size = 128;

    let mut writer = LobSnapshotWriter::new(&path, &config, HashMap::new()).unwrap();

    let actions = [Action::Add, Action::Trade, Action::Cancel, Action::Modify];

    for (i, action) in actions.iter().enumerate() {
        let mut state = make_test_state(2);
        state.sequence = i as u64;
        state.timestamp = Some((i as i64 + 1) * 1_000_000_000);
        state.triggering_action = Some(*action);
        state.bid_sizes[0] = (100 + i as u32 * 10) as u32;
        writer.write_snapshot(&state).unwrap();
    }
    let stats = writer.finish().unwrap();
    assert_eq!(stats.rows_written, 4);

    let file = fs::File::open(&path).unwrap();
    let reader = ParquetRecordBatchReaderBuilder::try_new(file)
        .unwrap()
        .build()
        .unwrap();

    let batches: Vec<_> = reader.into_iter().map(|b| b.unwrap()).collect();
    let total: usize = batches.iter().map(|b| b.num_rows()).sum();
    assert_eq!(total, 4);

    let batch = &batches[0];
    let seq = batch
        .column_by_name("sequence")
        .unwrap()
        .as_any()
        .downcast_ref::<UInt64Array>()
        .unwrap();
    for i in 0..4 {
        assert_eq!(seq.value(i), i as u64, "Sequence should match row index");
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ExportConfig tests
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_effective_levels_clamped() {
    let mut config = ExportConfig::default();
    config.levels = 50; // over MAX_LOB_LEVELS (20)
    assert_eq!(config.effective_levels(), 20);

    config.levels = 10;
    assert_eq!(config.effective_levels(), 10);

    config.levels = 1;
    assert_eq!(config.effective_levels(), 1);
}

#[test]
fn test_writer_rows_written_counter() {
    let tmp = TempDir::new("counter");
    let path = tmp.file("lob.parquet");
    let mut config = small_config(2, false);
    config.batch_size = 128;

    let state = make_test_state(2);
    let mut writer = LobSnapshotWriter::new(&path, &config, HashMap::new()).unwrap();

    assert_eq!(writer.rows_written(), 0);
    assert_eq!(writer.rows_seen(), 0);

    writer.write_snapshot(&state).unwrap();
    assert_eq!(writer.rows_written(), 1);

    writer.write_snapshot(&state).unwrap();
    assert_eq!(writer.rows_written(), 2);

    writer.finish().unwrap();
}
