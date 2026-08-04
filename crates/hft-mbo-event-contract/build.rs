use sha2::{Digest, Sha256};
use std::env;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

const CONTRACT_RELATIVE: &str = "contracts/mbo_event_contract.toml";
const DIGEST_RELATIVE: &str = "contracts/mbo_event_contract.sha256";
// Independent admission pin. A modified snapshot plus a recomputed sidecar is
// not sufficient to change the compiled contract. Updating this value requires
// a reviewed Rust source change as well as a root-authority change.
const EXPECTED_CONTRACT_SHA256: &str =
    "5e42cfd8e2d2f2a60e2e6152b1400e1b478a3c9354969ef2e85ac2618d36a302";

fn required_table<'a>(root: &'a toml::Value, name: &str) -> &'a toml::value::Table {
    root.get(name)
        .and_then(toml::Value::as_table)
        .unwrap_or_else(|| panic!("missing or invalid [{name}] table"))
}

fn required_str<'a>(table: &'a toml::value::Table, key: &str) -> &'a str {
    table
        .get(key)
        .and_then(toml::Value::as_str)
        .unwrap_or_else(|| panic!("missing or invalid string {key}"))
}

fn optional_str<'a>(table: &'a toml::value::Table, key: &str) -> Option<&'a str> {
    table.get(key).map(|value| {
        value
            .as_str()
            .unwrap_or_else(|| panic!("invalid optional string {key}"))
    })
}

fn required_integer(table: &toml::value::Table, key: &str) -> i64 {
    table
        .get(key)
        .and_then(toml::Value::as_integer)
        .unwrap_or_else(|| panic!("missing or invalid integer {key}"))
}

fn required_string_array<'a>(table: &'a toml::value::Table, key: &str) -> Vec<&'a str> {
    table
        .get(key)
        .and_then(toml::Value::as_array)
        .unwrap_or_else(|| panic!("missing or invalid string array {key}"))
        .iter()
        .map(|value| {
            value
                .as_str()
                .unwrap_or_else(|| panic!("non-string value in array {key}"))
        })
        .collect()
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn rust_string(value: &str) -> String {
    format!("{value:?}")
}

fn validate_digest_sidecar(path: &Path, expected: &str) {
    let text = fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
    let declared = text.trim();
    assert_eq!(
        declared.len(),
        64,
        "event contract sidecar must be one SHA-256"
    );
    assert!(
        declared
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()),
        "event contract sidecar must be lowercase hexadecimal"
    );
    assert_eq!(
        declared, expected,
        "event contract snapshot/sidecar mismatch"
    );
}

fn validate_closed_contract(root: &toml::Value) {
    let contract = required_table(root, "contract");
    assert_eq!(required_str(contract, "id"), "hft.canonical_mbo_event");
    assert_eq!(required_str(contract, "schema_version"), "1.0.0");
    assert_eq!(required_str(contract, "source_schema"), "mbo");
    assert_eq!(required_integer(contract, "expected_rtype"), 160);

    let layout = required_table(root, "record_layout");
    assert_eq!(required_integer(layout, "expected_record_size_bytes"), 56);
    assert_eq!(required_integer(layout, "expected_header_length_words"), 14);
    let expected_fields = [
        "source_object_sha256",
        "raw_ordinal",
        "subordinal",
        "rtype",
        "record_size_bytes",
        "publisher_id",
        "instrument_id",
        "ts_event",
        "ts_recv",
        "ts_in_delta",
        "channel_id",
        "sequence",
        "order_id",
        "price_raw",
        "size_raw",
        "flags_raw",
        "action_raw",
        "side_raw",
    ];
    assert_eq!(
        required_string_array(layout, "field_order"),
        expected_fields
    );
    let raw_fields = required_table(root, "raw_event_fields");
    assert_eq!(raw_fields.len(), expected_fields.len());
    let raw_ordinal = raw_fields["raw_ordinal"]
        .as_table()
        .expect("invalid raw_ordinal field descriptor");
    assert_eq!(
        required_str(raw_ordinal, "unit"),
        "one_based_decoded_record_index"
    );
    let source_digest = raw_fields["source_object_sha256"]
        .as_table()
        .expect("invalid source_object_sha256 field descriptor");
    assert_eq!(required_str(source_digest, "rust_type"), "Sha256DigestV1");
    assert_eq!(
        required_str(source_digest, "wire_encoding"),
        "lowercase_hex_64"
    );

    let source = required_table(root, "source_descriptor");
    assert_eq!(
        required_str(source, "dbn_ts_out_policy"),
        "must_be_false_for_v1"
    );

    let sentinels = required_table(root, "sentinels");
    assert_eq!(required_integer(sentinels, "price_undefined"), i64::MAX);
    // TOML integers are signed i64, so the u64 timestamp sentinel cannot be
    // represented through `as_integer`. Validate its exact source spelling.
    assert_eq!(
        required_integer(sentinels, "size_undefined"),
        u32::MAX as i64
    );
    assert_eq!(
        required_str(sentinels, "timestamp_undefined"),
        u64::MAX.to_string()
    );

    let expected_actions = [
        (
            "add",
            "A",
            65_i64,
            "book_command",
            "add",
            "add",
            "resting_order",
            &["A", "B"][..],
            &["order_id_nonzero", "price_present", "size_present_positive"][..],
        ),
        (
            "modify",
            "M",
            77,
            "book_command",
            "modify",
            "modify",
            "resting_order",
            &["A", "B"],
            &["order_id_nonzero", "price_present", "size_present_positive"],
        ),
        (
            "cancel",
            "C",
            67,
            "book_command",
            "cancel",
            "cancel",
            "resting_order",
            &["A", "B"],
            &["order_id_nonzero", "price_present", "size_present_positive"],
        ),
        (
            "clear",
            "R",
            82,
            "book_command",
            "clear",
            "clear",
            "none",
            &["N"],
            &[],
        ),
        (
            "trade",
            "T",
            84,
            "execution_carrier",
            "none",
            "trade_aggressor",
            "aggressor",
            &["A", "B", "N"],
            &["price_present", "size_present_positive"],
        ),
        (
            "fill",
            "F",
            70,
            "execution_carrier",
            "none",
            "resting_fill",
            "resting_filled_order",
            &["A", "B", "N"],
            &["price_present", "size_present_positive"],
        ),
        (
            "none",
            "N",
            78,
            "control",
            "none",
            "none",
            "none",
            &["A", "B", "N"],
            &[],
        ),
    ];
    let actions = required_table(root, "actions");
    assert_eq!(actions.len(), expected_actions.len());
    for (
        name,
        code,
        raw_byte,
        disposition,
        book_effect,
        lane,
        side_semantics,
        allowed_sides,
        required_fields,
    ) in expected_actions
    {
        let row = actions[name]
            .as_table()
            .unwrap_or_else(|| panic!("invalid action table {name}"));
        assert_eq!(row.len(), 8, "unexpected action fields for {name}");
        assert_eq!(required_str(row, "code"), code);
        assert_eq!(required_integer(row, "raw_byte"), raw_byte);
        assert_eq!(required_str(row, "disposition"), disposition);
        assert_eq!(required_str(row, "book_effect"), book_effect);
        assert_eq!(required_str(row, "lane"), lane);
        assert_eq!(required_str(row, "side_semantics"), side_semantics);
        assert_eq!(required_string_array(row, "allowed_sides"), allowed_sides);
        assert_eq!(
            required_string_array(row, "required_fields"),
            required_fields
        );
    }

    let expected_sides = [("ask", "A", 65_i64), ("bid", "B", 66), ("none", "N", 78)];
    let sides = required_table(root, "sides");
    assert_eq!(sides.len(), expected_sides.len());
    for (name, code, raw_byte) in expected_sides {
        let row = sides[name]
            .as_table()
            .unwrap_or_else(|| panic!("invalid side table {name}"));
        assert_eq!(row.len(), 2, "unexpected side fields for {name}");
        assert_eq!(required_str(row, "code"), code);
        assert_eq!(required_integer(row, "raw_byte"), raw_byte);
    }

    let expected_flags = [
        ("last", 128_i64),
        ("tob", 64),
        ("snapshot", 32),
        ("mbp", 16),
        ("bad_ts_recv", 8),
        ("maybe_bad_book", 4),
        ("publisher_specific", 2),
        ("reserved", 1),
    ];
    let flags = required_table(root, "flags");
    assert_eq!(flags.len(), expected_flags.len());
    for (name, mask) in expected_flags {
        let row = flags[name]
            .as_table()
            .unwrap_or_else(|| panic!("invalid flag table {name}"));
        assert_eq!(row.len(), 2, "unexpected flag fields for {name}");
        assert_eq!(required_integer(row, "mask"), mask);
    }

    let strict = required_table(root, "strict_validation");
    assert_eq!(
        required_str(strict, "snapshot_without_registered_policy"),
        "fatal"
    );
    assert_eq!(
        required_str(strict, "unassigned_flag_without_registered_policy"),
        "fatal"
    );

    let policies = required_table(root, "publisher_policies");
    assert_eq!(policies.len(), 2);
    let reject_all = policies["reject_all_v1"]
        .as_table()
        .expect("invalid reject_all_v1 publisher policy");
    assert_eq!(
        required_str(reject_all, "policy_id"),
        "reject_all_publisher_specific_v1"
    );
    assert_eq!(required_str(reject_all, "publisher_id_match"), "any");
    let xnas = policies["xnas_itch_historical_v1"]
        .as_table()
        .expect("invalid xnas_itch_historical_v1 publisher policy");
    assert_eq!(required_str(xnas, "policy_id"), "xnas_itch_historical_v1");
    assert_eq!(required_str(xnas, "dataset"), "XNAS.ITCH");
    assert_eq!(required_str(xnas, "publisher_id_match"), "allowlist");
}

fn generate(root: &toml::Value, contract_sha256: &str) -> String {
    let contract = required_table(root, "contract");
    let layout = required_table(root, "record_layout");
    let sentinels = required_table(root, "sentinels");
    let actions = required_table(root, "actions");
    let sides = required_table(root, "sides");
    let flags = required_table(root, "flags");
    let fields = required_table(root, "raw_event_fields");
    let policies = required_table(root, "publisher_policies");

    let mut code = String::with_capacity(16_000);
    writeln!(
        code,
        "// Generated into OUT_DIR from the crate-local contract snapshot."
    )
    .unwrap();
    writeln!(
        code,
        "// Source code is never modified by this build script.\n"
    )
    .unwrap();
    writeln!(
        code,
        "pub const CANONICAL_MBO_EVENT_CONTRACT_ID: &str = {};",
        rust_string(required_str(contract, "id"))
    )
    .unwrap();
    writeln!(
        code,
        "pub const CANONICAL_MBO_EVENT_SCHEMA_VERSION: &str = {};",
        rust_string(required_str(contract, "schema_version"))
    )
    .unwrap();
    writeln!(
        code,
        "pub const CANONICAL_MBO_EVENT_CONTRACT_SHA256: &str = {};",
        rust_string(contract_sha256)
    )
    .unwrap();
    writeln!(
        code,
        "pub const EXPECTED_MBO_RTYPE: u8 = {};",
        required_integer(contract, "expected_rtype")
    )
    .unwrap();
    writeln!(
        code,
        "pub const EXPECTED_MBO_RECORD_SIZE_BYTES: u16 = {};",
        required_integer(layout, "expected_record_size_bytes")
    )
    .unwrap();
    writeln!(code, "pub const FIXED_PRICE_SCALE: i64 = 1_000_000_000;").unwrap();
    writeln!(
        code,
        "pub const UNDEF_PRICE: i64 = {};",
        required_integer(sentinels, "price_undefined")
    )
    .unwrap();
    writeln!(
        code,
        "pub const UNDEF_ORDER_SIZE: u32 = {};",
        required_integer(sentinels, "size_undefined")
    )
    .unwrap();
    // u64::MAX is emitted from the closed v1 invariant because TOML's data
    // model exposes only signed 64-bit integers through `toml::Value`.
    writeln!(code, "pub const UNDEF_TIMESTAMP: u64 = u64::MAX;\n").unwrap();

    for (constant_name, table_name) in [
        ("PUBLISHER_POLICY_REJECT_ALL_V1_ID", "reject_all_v1"),
        (
            "PUBLISHER_POLICY_XNAS_ITCH_HISTORICAL_V1_ID",
            "xnas_itch_historical_v1",
        ),
    ] {
        let row = policies[table_name].as_table().unwrap();
        writeln!(
            code,
            "pub const {constant_name}: &str = {};",
            rust_string(required_str(row, "policy_id"))
        )
        .unwrap();
    }
    writeln!(code).unwrap();

    for name in ["add", "modify", "cancel", "clear", "trade", "fill", "none"] {
        let row = actions[name].as_table().unwrap();
        writeln!(
            code,
            "pub const ACTION_{}: u8 = {};",
            name.to_ascii_uppercase(),
            required_integer(row, "raw_byte")
        )
        .unwrap();
    }
    writeln!(code).unwrap();
    for name in ["ask", "bid", "none"] {
        let row = sides[name].as_table().unwrap();
        writeln!(
            code,
            "pub const SIDE_{}: u8 = {};",
            name.to_ascii_uppercase(),
            required_integer(row, "raw_byte")
        )
        .unwrap();
    }
    writeln!(code).unwrap();
    for name in [
        "last",
        "tob",
        "snapshot",
        "mbp",
        "bad_ts_recv",
        "maybe_bad_book",
        "publisher_specific",
        "reserved",
    ] {
        let row = flags[name].as_table().unwrap();
        writeln!(
            code,
            "pub const FLAG_{}: u8 = {};",
            name.to_ascii_uppercase(),
            required_integer(row, "mask")
        )
        .unwrap();
    }
    writeln!(code, "pub const KNOWN_FLAG_MASK: u8 = 0xFF;\n").unwrap();

    writeln!(
        code,
        "pub const RAW_EVENT_FIELD_DESCRIPTORS: &[RawEventFieldDescriptor] = &["
    )
    .unwrap();
    for name in required_string_array(layout, "field_order") {
        let row = fields[name]
            .as_table()
            .unwrap_or_else(|| panic!("invalid raw field {name}"));
        writeln!(code, "    RawEventFieldDescriptor {{").unwrap();
        writeln!(code, "        name: {},", rust_string(name)).unwrap();
        writeln!(
            code,
            "        rust_type: {},",
            rust_string(required_str(row, "rust_type"))
        )
        .unwrap();
        writeln!(
            code,
            "        unit: {},",
            rust_string(required_str(row, "unit"))
        )
        .unwrap();
        writeln!(
            code,
            "        origin: {},",
            rust_string(required_str(row, "origin"))
        )
        .unwrap();
        writeln!(
            code,
            "        clock: {},",
            optional_str(row, "clock")
                .map(|value| format!("Some({})", rust_string(value)))
                .unwrap_or_else(|| "None".to_string())
        )
        .unwrap();
        writeln!(
            code,
            "        wire_encoding: {},",
            optional_str(row, "wire_encoding")
                .map(|value| format!("Some({})", rust_string(value)))
                .unwrap_or_else(|| "None".to_string())
        )
        .unwrap();
        writeln!(code, "    }},").unwrap();
    }
    writeln!(code, "];").unwrap();
    code
}

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let contract_path = manifest_dir.join(CONTRACT_RELATIVE);
    let digest_path = manifest_dir.join(DIGEST_RELATIVE);
    println!("cargo:rerun-if-changed={}", contract_path.display());
    println!("cargo:rerun-if-changed={}", digest_path.display());
    println!("cargo:rerun-if-changed=build.rs");

    let contract_bytes = fs::read(&contract_path)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", contract_path.display()));
    let digest = sha256_hex(&contract_bytes);
    validate_digest_sidecar(&digest_path, &digest);
    assert_eq!(
        digest, EXPECTED_CONTRACT_SHA256,
        "event contract snapshot does not match the independently pinned authority digest"
    );
    let root: toml::Value = std::str::from_utf8(&contract_bytes)
        .expect("event contract must be UTF-8")
        .parse()
        .expect("event contract must be valid TOML");
    validate_closed_contract(&root);
    let generated = generate(&root, &digest);

    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    fs::write(out_dir.join("mbo_event_contract_generated.rs"), generated)
        .expect("write generated contract into OUT_DIR");
}
