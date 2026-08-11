# Current limitations and operational warnings

This file records live version-1 limitations. Historical measurements and
superseded failure explanations remain available in Git history and must not be
used as current operating guidance.

## Qualified versus compatibility output

Only the strict XNAS path can attest source identity, envelope causality,
transactional mutation, ordinal custody, and terminal population
reconciliation. `DbnBridge` and `LobReconstructor` are
compatibility primitives. Their successful return is not a qualified replay or
authorization to publish a reusable artifact.

## Publication is external

This repository deliberately contains no Parquet publisher, hot-store resolver,
or multi-artifact generation transaction. The extractor must verify a terminal
strict replay/equivalence receipt and then perform one atomic, identity-bound
publication transaction. An incomplete staging directory is non-consumable.

## Invalid terminal intervals

An identity may have earlier valid epochs and still end `INVALID` after a
quarantined anomaly or EOF tail. Its replay receipt describes both selected and
excluded populations; it does not make a complete-day dataset admissible.
Publication policy must declare whether it requires a terminally valid identity
or intentionally publishes named valid epochs only.

## Compatibility anomaly counters

The compatibility `LobStats` counters describe that compatibility book only.
They do not replace strict quarantine reasons, validity epochs, or terminal
receipts. The exact `3.0.0` JSON envelope requires every field; missing or
unknown fields and all other schema versions are rejected.

## Performance

Strict observation commitments hash the complete committed snapshot and add
measurable cost relative to the pre-v1 compatibility replay. Do not weaken the
commitment to recover throughput. Optimize only through a parity-proven
implementation whose terminal commitments match an independent reference.

## Consumer migration

Version 1 intentionally removes ambiguous and failure-erasing APIs. A consumer
that still imports an unqualified file iterator, hot-store manager, lifecycle/queue
analytics, trade aggregator, or Parquet writers is incompatible and must fail
at dependency resolution or compile time; it must not receive a shim that
silently preserves the old behavior.
