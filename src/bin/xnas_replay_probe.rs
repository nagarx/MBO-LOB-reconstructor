//! Development-only, stdout-at-EOF probe for the strict XNAS replay.
//!
//! The expectation file must be minted from an independently validated
//! catalog/custody owner. This binary re-verifies the opened bytes and emits
//! nothing on stdout until the physical source reaches verified EOF. A
//! qualified terminal receipt exits 0; a source-complete semantic
//! disqualification is serialized as explicitly non-consumable JSON and exits
//! 2; source, resource, or software failures emit no stdout and exit 1.

use hft_mbo_event_contract::Sha256DigestV1;
use mbo_lob_reconstructor::{
    XnasReplayErrorV1, XnasReplayProbeRequestV1, XnasReplayRunV1, XnasTerminalDisqualificationV1,
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::error::Error;
use std::fs;
use std::path::PathBuf;

#[derive(Serialize)]
struct ProbeOutputV3 {
    schema: &'static str,
    authority: &'static str,
    expectation_file_sha256: Sha256DigestV1,
    expectation: XnasReplayProbeRequestV1,
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
    let expectation: XnasReplayProbeRequestV1 = serde_json::from_slice(&expectation_bytes)?;
    let replay = expectation.open_admitted_strict_replay_unbound_development()?;
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
