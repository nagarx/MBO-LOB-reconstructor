use hft_mbo_event_contract::{LogicalSourceV1, Sha256DigestV1};
use mbo_lob_reconstructor::{
    CanonicalSourceExpectationV1, XnasDailyMetadataExpectationV1, XnasExpectedInstrumentIdentityV1,
};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;

pub const START_NS: u64 = 1_751_328_000_000_000_000;
pub const END_NS: u64 = START_NS + 86_400_000_000_000;

#[allow(dead_code)]
pub fn xnas_metadata_expectation(
    source: &CanonicalSourceExpectationV1,
    instruments: &[(u16, u32, &str)],
) -> XnasDailyMetadataExpectationV1 {
    let instruments = instruments
        .iter()
        .map(|&(publisher_id, instrument_id, symbol)| {
            XnasExpectedInstrumentIdentityV1::new(publisher_id, instrument_id, symbol).unwrap()
        })
        .collect();
    XnasDailyMetadataExpectationV1::new(
        source.logical().compressed_sha256,
        source.logical().metadata_start_ns,
        source.logical().metadata_end_ns,
        "2025-07-01",
        instruments,
    )
    .unwrap()
}

pub fn digest_and_len(path: &Path) -> (Sha256DigestV1, u64) {
    let bytes = fs::read(path).unwrap();
    (
        Sha256DigestV1::from_bytes(Sha256::digest(&bytes).into()),
        bytes.len() as u64,
    )
}

pub fn expectation(
    path: &Path,
    dataset: &str,
    schema: &str,
    version: u8,
    ts_out: bool,
    expected_records: u64,
) -> CanonicalSourceExpectationV1 {
    expectation_with_population(
        path,
        dataset,
        schema,
        version,
        ts_out,
        expected_records,
        "TEST",
        1,
        1,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn expectation_with_population(
    path: &Path,
    dataset: &str,
    schema: &str,
    version: u8,
    ts_out: bool,
    expected_records: u64,
    requested_symbols_preview: &str,
    symbols_n: u64,
    active_instruments_n: u64,
) -> CanonicalSourceExpectationV1 {
    // macOS exposes /var as a symlink to /private/var. The production loader
    // intentionally rejects every symlink component, so synthetic fixtures use
    // the physical temporary-directory coordinate.
    let root = path.parent().unwrap().canonicalize().unwrap();
    let relative_path = path.file_name().unwrap().to_str().unwrap();
    let (digest, bytes) = digest_and_len(path);
    let content = json!({
        "release": {
            "release_id": "synthetic-test-v1",
            "storage_root_id": "synthetic-test-root",
            "observed_root_path": root.to_str().unwrap(),
            "canonical_profile_sha256": "11".repeat(32),
            "embedded_per_file_tsv_sha256": "12".repeat(32),
            "evidence_manifest_sha256": "13".repeat(32),
            "terminal_validation_receipt_sha256": "14".repeat(32),
            "terminal_validation_status": "PASS"
        },
        "groups": {
            "synthetic": {
                "identity": {
                    "dataset": dataset,
                    "schema": "Mbo",
                    "dbn_version": version,
                    "requested_symbols_preview": requested_symbols_preview,
                    "requested_symbols_sha256": "15".repeat(32),
                    "symbols_n": symbols_n,
                    "active_instruments_n": active_instruments_n
                },
                "provider_receipt": {
                    "relative_path": "manifest.json",
                    "sha256": "16".repeat(32),
                    "job_id": "synthetic-job",
                    "declared_data_file_count": 1
                },
                "objects": [{
                    "relative_path": relative_path,
                    "compressed_sha256": digest.to_hex(),
                    "compressed_bytes": bytes,
                    "records": expected_records,
                    "dataset": dataset,
                    "schema": "Mbo",
                    "dbn_version": version,
                    "metadata_start_ns": START_NS,
                    "metadata_end_ns": END_NS,
                    "requested_symbols_preview": requested_symbols_preview,
                    "requested_symbols_sha256": "15".repeat(32),
                    "symbols_n": symbols_n,
                    "active_instruments_n": active_instruments_n,
                    "provenance_tier": "SYNTHETIC_TEST_ONLY"
                }]
            }
        }
    });
    let content_bytes = serde_json::to_vec(&content).unwrap();
    let content_sha256 = Sha256DigestV1::from_bytes(Sha256::digest(&content_bytes).into());
    let envelope = json!({
        "schema": "mbo_backbone_hashed_envelope_v1",
        "content": content,
        "content_sha256": content_sha256.to_hex()
    });
    let projection_path = root.join(format!("{relative_path}.custody.json"));
    let projection_bytes = serde_json::to_vec_pretty(&envelope).unwrap();
    fs::write(&projection_path, &projection_bytes).unwrap();
    let projection_file_sha256 =
        Sha256DigestV1::from_bytes(Sha256::digest(&projection_bytes).into());
    CanonicalSourceExpectationV1::new(
        LogicalSourceV1 {
            catalog_release_id: "synthetic-test-v1".into(),
            catalog_storage_root_id: "synthetic-test-root".into(),
            custody_projection_schema: "mbo_backbone_hashed_envelope_v1".into(),
            custody_projection_file_sha256: projection_file_sha256,
            custody_projection_content_sha256: content_sha256,
            canonical_profile_sha256: Sha256DigestV1::from_bytes([0x11; 32]),
            embedded_per_file_tsv_sha256: Sha256DigestV1::from_bytes([0x12; 32]),
            evidence_manifest_sha256: Sha256DigestV1::from_bytes([0x13; 32]),
            terminal_validation_receipt_sha256: Sha256DigestV1::from_bytes([0x14; 32]),
            terminal_validation_status: "PASS".into(),
            relative_path: relative_path.into(),
            compressed_sha256: digest,
            compressed_bytes: bytes,
            expected_records,
            metadata_start_ns: START_NS,
            metadata_end_ns: END_NS,
            requested_symbols_preview: requested_symbols_preview.into(),
            requested_symbols_sha256: Sha256DigestV1::from_bytes([0x15; 32]),
            symbols_n,
            active_instruments_n,
            provenance_tier: "SYNTHETIC_TEST_ONLY".into(),
            provider_manifest_relative_path: "manifest.json".into(),
            provider_manifest_sha256: Sha256DigestV1::from_bytes([0x16; 32]),
            provider_job_id: "synthetic-job".into(),
            provider_declared_data_file_count: 1,
            dbn_version: version,
            dbn_ts_out: ts_out,
            dataset: dataset.into(),
            schema: schema.into(),
        },
        projection_path,
        root,
    )
    .unwrap()
}

#[allow(dead_code)]
pub fn probe_request_value(path: &Path, expected_records: u64) -> serde_json::Value {
    let expectation = expectation(path, "XNAS.ITCH", "mbo", 1, false, expected_records);
    let mut value = serde_json::to_value(expectation.logical()).unwrap();
    let object = value.as_object_mut().unwrap();
    object.insert(
        "custody_projection_path".into(),
        expectation
            .custody_projection_path()
            .to_str()
            .unwrap()
            .into(),
    );
    object.insert(
        "storage_root_path".into(),
        expectation.storage_root_path().to_str().unwrap().into(),
    );
    object.insert("snapshot_depth".into(), 10.into());
    object.insert("max_envelope_members".into(), 64.into());
    object.insert("max_sequence_blocks".into(), 64.into());
    value
}
