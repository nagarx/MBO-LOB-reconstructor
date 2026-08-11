# MBO-LOB-reconstructor

Source-bound Databento MBO decoding and exact XNAS order-book reconstruction.

Version 1 separates two different capabilities:

- The strict XNAS replay API is the only surface that can attest a qualified
  reconstruction. It binds the configured source identity to the opened bytes,
  preserves raw event semantics, commits complete sequence envelopes
  transactionally, and emits a terminal custody receipt.
- `LobReconstructor`, `DbnBridge`, and the general analytics helpers are
  compatibility or exploratory primitives. They are useful for tests and local
  analysis, but they do not establish source identity, XNAS envelope causality,
  or artifact admissibility.

No API in this crate publishes a production generation. Publication,
cross-artifact reconciliation, and generation atomicity belong to the consuming
extractor.

## Event semantics

The public action enum preserves Databento's raw action distinction:

| Raw action | Rust action | Strict semantic lane | Exact book mutation |
| --- | --- | --- | --- |
| `A` | `Action::Add` | book command | add order |
| `C` | `Action::Cancel` | book command | reduce or remove order |
| `M` | `Action::Modify` | book command | replace order state |
| `R` | `Action::Clear` | reset control | clear/requalify under policy |
| `T` | `Action::TradeAggregate` | trade-print carrier | none |
| `F` | `Action::Fill` | resting-fill carrier | none |
| `N` | `Action::None` | explicit no-op control | none |

For XNAS normalized MBO, the authoritative resting-order reduction is `C`.
Applying `T` or `F` to the book would double count an execution. `T` and `F`
remain available to analytical consumers with their distinct side semantics.

## Strict replay

The strict path is built around these types:

- `StrictDbnLoaderV1`: verifies the opened file and DBN metadata against a
  complete expected source identity before yielding records.
- `hft_mbo_event_contract::RawMboEventV1`: lossless raw carrier representation.
- `VerifiedStreamEventV1`: accepted or rejected event with exact ordinal
  custody; invalid records are not silently dropped.
- `StrictXnasReplayV1`: per-identity envelope, lifecycle, quarantine, recovery,
  and exact-book owner.
- `XnasQualifiedReplayPlanV1`: immutable two-pass plan binding source and replay
  configuration.
- `XnasCommittedObservationAccumulatorV1`: reconstructor-owned in-memory
  verifier that independently accounts for every observation consumed by a
  downstream private staging pass and can close only with the post-EOF
  `XnasReplayEquivalenceReceiptV1` for the exact qualification receipt.
- `XnasReplayReceiptV1`: terminal record/population/identity reconciliation.

The XNAS causal observation boundary is the committed sequence envelope. Its
`effective_available_ns` is later than or equal to the envelope endpoint and is
derived from the closure witness. The witness is not included in the envelope
it closes.

See [docs/STRICT_XNAS_REPLAY_CONTRACT.md](docs/STRICT_XNAS_REPLAY_CONTRACT.md)
for the complete state and failure contract.

## Failure ownership

The strict replay distinguishes three outcomes:

1. Source/custody/capacity/internal failures are replay-fatal. No success
   receipt can be minted.
2. Identity-attributable market-data anomalies quarantine the affected
   identity and enter `INVALID`; ordinary traffic cannot restore validity.
3. A clean, witnessed `R` recovery envelope may start a new validity epoch.

An identity that was valid earlier can end with an invalid EOF interval and
still obtain a replay receipt describing the excluded population. That receipt
does not by itself authorize a complete-day published artifact. The extractor's
publication policy must inspect terminal status, validity epochs, and quarantine
populations explicitly.

## Compatibility surface

There is no pathname-only compatibility file iterator in v1. Physical EOF
cannot prove that a source object is complete, so file-backed consumers must
provide a `CanonicalSourceExpectationV1` and use the strict loader/replay path.

`LobReconstructor` is an in-memory compatibility book. It preserves T/F book
no-op behavior and exposes strict JSON config/stat schemas, but it does not bind
physical input identity or XNAS envelope timing. Do not use it to claim a
qualified source-bound reconstruction.

The old queue-position, order-lifecycle, and trade-aggregation modules were
removed from the v1 public surface because they collapsed T and F semantics.
The old hot-store and Parquet-export surfaces were also removed: the former
could substitute basename-matched bytes without source-content custody, while
the latter could leave partial or falsely identified success-shaped output.
Git history is the migration reference for those retired APIs.

## Features and binaries

| Cargo feature | Default | Purpose |
| --- | --- | --- |
| `databento` | yes | DBN decoding, source hashing, and strict XNAS replay |

Supported binaries:

- `xnas_replay_probe`: quarantined strict-replay evidence probe.

The crate does not prepare or resolve decompressed caches and does not write
Parquet. A caller may open any physical DBN path, including a separately
prepared decompressed file, only by naming and verifying that exact source in a
`CanonicalSourceExpectationV1`.

## Build and verification

```bash
cargo test --workspace --all-features --all-targets
cargo clippy --workspace --all-targets --all-features -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --all-features --no-deps
```

For a qualified real-data replay, supply a complete
`CanonicalSourceExpectationV1` including the configured path, expected SHA-256,
dataset/schema/publisher identity, instrument mapping, and source date. Do not
infer these fields from a filename.

## Ownership boundaries

- This repository owns raw-to-canonical MBO event semantics, exact XNAS book
  mutation, replay lifecycle, and reconstruction custody receipts.
- The feature extractor owns sampling, feature/target layouts, normalization,
  row alignment, artifact metadata, and atomic generation publication.
- The root executable contract owns cross-repository schema identity.

See [CODEBASE.md](CODEBASE.md) for the module inventory and
[ARCHITECTURE.md](ARCHITECTURE.md) for the design rationale.
