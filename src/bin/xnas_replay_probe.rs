//! Development-only, stdout-at-EOF probe for the strict XNAS replay.
//!
//! The expectation file must be minted from an independently validated
//! catalog/custody owner. This binary re-verifies the opened bytes and emits
//! nothing on stdout until the physical source reaches verified EOF. A
//! qualified terminal receipt exits 0; a source-complete semantic
//! disqualification is serialized as explicitly non-consumable JSON and exits
//! 2; source, resource, or software failures emit no stdout and exit 1.

use hft_mbo_event_contract::{LogicalSourceV1, PublisherPolicyIdV1, Sha256DigestV1};
use mbo_lob_reconstructor::{
    CanonicalSourceExpectationV1, StrictDbnLoaderV1, StrictXnasReplayV1, XnasReplayConfigV1,
    XnasReplayErrorV1, XnasReplayRunV1, XnasTerminalDisqualificationV1,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::error::Error;
use std::fs;
use std::num::NonZeroUsize;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProbeExpectationV1 {
    catalog_release_id: String,
    catalog_content_sha256: Sha256DigestV1,
    catalog_object_id: String,
    canonical_path: String,
    canonical_sha256: Sha256DigestV1,
    canonical_bytes: u64,
    expected_records: u64,
    dbn_version: u8,
    dbn_ts_out: bool,
    dataset: String,
    schema: String,
    snapshot_depth: usize,
    max_envelope_members: usize,
    max_sequence_blocks: usize,
}

#[derive(Serialize)]
struct ProbeOutputV3 {
    schema: &'static str,
    authority: &'static str,
    expectation_file_sha256: Sha256DigestV1,
    expectation: ProbeExpectationV1,
    outcome: ProbeTerminalOutcomeV1,
}

#[derive(Serialize)]
#[serde(tag = "qualification", rename_all = "snake_case")]
enum ProbeTerminalOutcomeV1 {
    Qualified {
        run: XnasReplayRunV1,
    },
    Disqualified {
        diagnostic: XnasTerminalDisqualificationV1,
    },
}

fn main() {
    match run() {
        Ok(true) => {}
        Ok(false) => std::process::exit(2),
        Err(error) => {
            eprintln!("xnas_replay_probe failed: {error}");
            std::process::exit(1);
        }
    }
}

fn run() -> Result<bool, Box<dyn Error>> {
    let mut arguments = std::env::args_os().skip(1);
    let expectation_path = arguments
        .next()
        .ok_or("usage: xnas_replay_probe <expectation-v1.json> [selected-raw-ordinals-csv]")?;
    let selected = arguments
        .next()
        .map(|value| parse_ordinals(&value.to_string_lossy()))
        .transpose()?
        .unwrap_or_default();
    if arguments.next().is_some() {
        return Err("unexpected extra command-line argument".into());
    }

    let expectation_bytes = fs::read(PathBuf::from(expectation_path))?;
    let expectation_file_sha256 =
        Sha256DigestV1::from_bytes(Sha256::digest(&expectation_bytes).into());
    let expectation: ProbeExpectationV1 = serde_json::from_slice(&expectation_bytes)?;
    validate_expectation(&expectation)?;
    let source = CanonicalSourceExpectationV1::new(
        LogicalSourceV1 {
            catalog_release_id: expectation.catalog_release_id.clone(),
            catalog_object_id: expectation.catalog_object_id.clone(),
            canonical_path: expectation.canonical_path.clone(),
            canonical_sha256: expectation.canonical_sha256,
            canonical_bytes: expectation.canonical_bytes,
            dbn_version: expectation.dbn_version,
            dbn_ts_out: expectation.dbn_ts_out,
            dataset: expectation.dataset.clone(),
            schema: expectation.schema.clone(),
        },
        expectation.expected_records,
    );
    let stream = StrictDbnLoaderV1::open(
        source,
        &expectation.canonical_path,
        PublisherPolicyIdV1::XnasItchHistorical,
    )?;
    let replay = StrictXnasReplayV1::from_strict_stream(
        stream,
        XnasReplayConfigV1::new(
            NonZeroUsize::new(expectation.snapshot_depth)
                .ok_or("snapshot_depth must be nonzero")?,
            NonZeroUsize::new(expectation.max_envelope_members)
                .ok_or("max_envelope_members must be nonzero")?,
            NonZeroUsize::new(expectation.max_sequence_blocks)
                .ok_or("max_sequence_blocks must be nonzero")?,
        ),
    )?;
    let (outcome, qualified) = match replay.run_to_eof_with_selected_ordinals(&selected) {
        Ok(run) => (ProbeTerminalOutcomeV1::Qualified { run }, true),
        Err(XnasReplayErrorV1::TerminalDisqualified(diagnostic)) => (
            ProbeTerminalOutcomeV1::Disqualified {
                diagnostic: *diagnostic,
            },
            false,
        ),
        Err(error) => return Err(error.into()),
    };
    let output = ProbeOutputV3 {
        schema: "xnas_strict_replay_probe_v3",
        authority: "development_only_authorizes_nothing",
        expectation_file_sha256,
        expectation,
        outcome,
    };
    serde_json::to_writer(std::io::stdout().lock(), &output)?;
    println!();
    Ok(qualified)
}

fn validate_expectation(value: &ProbeExpectationV1) -> Result<(), Box<dyn Error>> {
    if value.catalog_release_id.is_empty()
        || value.catalog_content_sha256.is_zero()
        || value.catalog_object_id.is_empty()
        || value.canonical_path.is_empty()
        || value.canonical_bytes == 0
        || value.expected_records == 0
        || value.snapshot_depth == 0
        || value.max_envelope_members == 0
        || value.max_sequence_blocks == 0
        || value.dataset != "XNAS.ITCH"
        || value.schema != "mbo"
        || value.dbn_ts_out
    {
        return Err("expectation is incomplete or outside the strict XNAS MBO v1 profile".into());
    }
    Ok(())
}

fn parse_ordinals(value: &str) -> Result<BTreeSet<u64>, Box<dyn Error>> {
    if value.is_empty() {
        return Ok(BTreeSet::new());
    }
    value
        .split(',')
        .map(|item| {
            let ordinal = item.parse::<u64>()?;
            if ordinal == 0 {
                return Err("selected raw ordinals are one-based".into());
            }
            Ok(ordinal)
        })
        .collect()
}
