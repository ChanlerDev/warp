# ADR-001: Harden FlatStorage Row Ingestion

## Status

Accepted

## Context

`FlatStorage::push_rows_internal` used unchecked index construction under the assumption that all
incoming rows matched `FlatStorage.columns` and contained valid wide-character pairs. Apple crash
logs prove that assumption can fail in production and later crash `RowIterator::next`.

## Decision

Keep the unchecked path for normal rows. Detect anomalous rows at ingestion and use checked
`EntryBuilder` reflow for those rows. Add a defensive bound check in `RowIterator` so previously
corrupted in-memory indexes degrade with a warning instead of terminating Warp.

## Consequences

Normal scrollback writes keep their existing fast path. Exceptional rows incur checked reflow.
Corrupted historical content may render with dropped wide-character flags at the damaged boundary,
but the application remains available.
