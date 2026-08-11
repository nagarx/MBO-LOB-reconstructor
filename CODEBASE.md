# Codebase map

This document describes the live version-1 source tree. Source code and tests
remain authoritative when this map drifts.

## Repository role

The repository owns four things:

1. A lossless raw MBO event carrier and source-bound semantic classification.
2. Verified DBN file identity, metadata, ordinal, byte, and population custody.
3. Exact XNAS sequence-envelope reconstruction with transactional book updates.
4. Terminal receipts and bounded traces that downstream code can independently
   reconcile.

It does not own feature formulas, label construction, normalization,
producer-consumer generation publication, or research-result admissibility.

## Workspace

```text
.
|-- build.rs
|-- crates/
|   `-- hft-mbo-event-contract/
|-- docs/
|   `-- STRICT_XNAS_REPLAY_CONTRACT.md
|-- examples/
|-- src/
|   |-- analytics.rs
|   |-- bin/
|   |   `-- xnas_replay_probe.rs
|   |-- canonical_dbn.rs
|   |-- constants.rs
|   |-- dbn_bridge.rs
|   |-- error.rs
|   |-- lib.rs
|   |-- loader/
|   |   |-- canonical.rs
|   |   `-- mod.rs
|   |-- lob/
|   |   |-- day_boundary.rs
|   |   |-- mod.rs
|   |   |-- multi_symbol.rs
|   |   |-- price_level.rs
|   |   `-- reconstructor.rs
|   |-- source.rs
|   |-- statistics.rs
|   |-- types.rs
|   |-- warnings.rs
|   `-- xnas/
|       |-- book.rs
|       |-- diagnostics.rs
|       |-- envelope.rs
|       |-- mod.rs
|       `-- qualified.rs
`-- tests/
```

## Module inventory

### `crates/hft-mbo-event-contract`

Authoritative raw-event and source-policy contract.

- `RawMboEventV1` preserves every DBN MBO field without interpreting it.
- `AcceptedMboEventV1` and `RejectedMboEventV1` make validation outcomes typed.
- `EventDispositionV1` separates book commands, execution carriers, resets, and
  explicit controls.
- `SourcePolicyBindingV1` binds publisher-specific semantics to a registered
  source policy.
- The generated contract identity is checked against the repository-local
  snapshot and digest sidecar at build and test time.

The crate must not perform book mutation, feature computation, sampling, or
publication.

### `src/canonical_dbn.rs`

Projects `dbn::MboMsg` into `RawMboEventV1`. Projection is lossless: sentinels,
flags, raw action/side bytes, and clocks are preserved for later policy-bound
validation.

### `src/loader/canonical.rs`

Owns the strict physical source boundary.

- Opens the exact configured path.
- Hashes and stats the opened file.
- Reads DBN metadata with an explicit `VersionUpgradePolicy::AsIs`.
- Verifies dataset, schema, publisher, instrument mapping, date, and expected
  digest before yielding accepted data.
- Assigns strictly increasing raw ordinals.
- Reconciles attempted, accepted, rejected, byte, and record populations in a
  terminal `CanonicalReadReceiptV1`.
- Fuses after a source-level failure; a caller cannot continue and later mint a
  clean receipt.

### `src/loader/mod.rs`

Strict-loader facade and shared byte-counting reader. There is no unqualified
pathname-only DBN iterator in the v1 public surface.

### `src/dbn_bridge.rs`

Compatibility projection from DBN MBO to `MboMessage`.

- `T` maps to `Action::TradeAggregate`.
- `F` maps to `Action::Fill`.
- side `S` is rejected rather than coerced to ask.
- timestamp zero is preserved; values outside the public timestamp type fail
  with the exact unsigned value.
- batch conversion stops at the first error.

### `src/xnas/envelope.rs`

Owns XNAS terminal sequence-envelope formation.

- Envelopes are identity-local and source/ordinal contiguous under the declared
  grammar.
- A different sequence member is the closure witness for a terminal envelope.
- `effective_available_ns` is the causal observation time.
- The witness is not a member of the envelope it closes.
- Duplicate, regressing, inconsistent, or over-capacity candidates fail with a
  stable typed reason.

### `src/xnas/book.rs`

Exact order and aggregate book owner.

- A/C/M/R are the only book-changing carriers.
- T/F are retained in the committed envelope but cannot mutate the book.
- Every ready envelope is applied to a private transaction.
- Endpoint validation and whole-book reconciliation happen before commit.
- A failed transaction changes no live order, level, counter, epoch, or digest.
- State digests bind complete order/level values and reset epoch, not only BBO.

### `src/xnas/mod.rs`

Strict replay orchestrator and per-identity lifecycle owner.

Identity states are:

- `Uninitialized`
- `AwaitingFirstQualifiedEnvelope`
- `Valid`
- `Invalid`
- `Recovering`
- `InvalidAfterEofTail`

Identity-attributable data failures quarantine the candidate and witness,
invalidate only that identity, and preserve other identities. Ordinary traffic
cannot restore validity. A clean `R` envelope plus clean witness can establish a
new validity epoch. Source/custody/capacity/arithmetic/internal failures remain
replay-fatal.

The replay exposes committed observations, stable diagnostic populations,
per-identity terminal status, and a terminal receipt. A replay receipt attests
the selected population; downstream publication must still decide whether an
invalid terminal interval is admissible for its declared dataset.

### `src/xnas/qualified.rs`

Immutable strict-replay plan and two-pass equivalence owner. It binds source
expectation, replay configuration, build identity, and digest algorithm. The
planning and execution passes must produce identical terminal commitments.
`XnasCommittedObservationAccumulatorV1` independently recomputes the chain for
the exact observations a downstream private staging path consumed, rejects a
post-EOF equivalence receipt for a different qualification, and returns only a
non-serializable in-memory closure token after count and terminal-chain
reconciliation. A qualification receipt alone cannot close the accumulator.

### `src/lob/reconstructor.rs`

Compatibility in-memory book and snapshot API.

- T and F are book no-ops; C performs the resting reduction.
- malformed commands are not reclassified as controls from field shape.
- configuration uses `deny_unknown_fields` and rejects legacy semantic names.
- stats use an exact `3.0.0` envelope; schema-less, old-name, and unknown-version
  payloads are rejected.
- stats writes use same-directory temporary files, file sync, atomic persist,
  and parent-directory sync. There is no direct-write fallback.

This module is not a substitute for source-bound XNAS replay.

### `src/types.rs`

Compatibility `Action`, `Side`, `MboMessage`, and fixed-capacity `LobState`
types. It intentionally preserves `TradeAggregate` and `Fill` as different
actions.

### `src/source.rs`

In-memory/synthetic `MarketDataSource`, `SourceMetadata`, and `VecSource`.
Physical DBN was removed from this infallible iterator abstraction because it
cannot carry typed decode failures or prove file identity.

### Compatibility analytics

`analytics.rs`, `statistics.rs`, `warnings.rs`, `lob/day_boundary.rs`, and
`lob/multi_symbol.rs` support exploratory or compatibility workflows. They do
not mint strict replay receipts. The old queue-position, order-lifecycle, and
trade-aggregation modules were deleted because their public semantics collapsed
T and F.

## Public binaries

### `xnas_replay_probe`

Executes the qualified replay plan and writes quarantined evidence JSON. It is a
validation instrument, not a production feature-generation or publication
command.

## Test surfaces

| Surface | Main responsibility |
| --- | --- |
| unit tests in `hft-mbo-event-contract` | raw-field preservation, all flag bytes, action/side policy, source binding |
| `tests/strict_dbn_boundary.rs` | file/metadata/digest/ordinal/reconciliation failures |
| `tests/xnas_replay.rs` | envelope timing, transactional mutation, quarantine, recovery, EOF, two-pass receipts |
| `tests/lob_stats_counters.rs` | compatibility counter semantics and JSON schema |
| other unit/integration tests | compatibility calculations and regression coverage |

Required repository gates:

```bash
cargo test --workspace --all-features --all-targets
cargo clippy --workspace --all-targets --all-features -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --all-features --no-deps
```

Passing these gates is necessary, not sufficient. Live-data acceptance also
requires an independently identified source, strict terminal receipt,
population reconciliation, and downstream contract verification.

## Change rules

- Do not merge T and F into one action or infer action identity from field
  shape.
- Do not add warning/drop, fallback-path, or success-after-partial-input APIs.
- Do not make raw timestamps the feature-availability clock.
- Do not expose mutable staged envelopes or books before transactional commit.
- Do not treat a replay receipt as a published-generation receipt.
- Add a failing counterexample and an independent acceptance channel for every
  semantic change.
