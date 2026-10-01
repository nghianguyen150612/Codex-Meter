# ADR-002: Versioned data contracts

## Status

Accepted

## Context

Rust collection and Python analytics require one precise vocabulary for missingness, units, privacy, lifecycle, and independently valid evidence. SQLite is durable storage but is not an adequate process or domain contract, and source/provider behavior remains partially unknown.

## Decision

Use JSON Schema Draft 2020-12. Store the initial contracts beneath `schemas/v1/`, with repository-relative references to shared definitions and no fabricated external schema host. Every top-level durable/process-boundary object carries an explicit semantic `schema_version`.

Objects are strict and privacy-safe. Unknown/unavailable values and observed/derived provenance are explicit. Raw tokens, future weighted/effective metrics, and percentage-point quota evidence remain structurally separate. Token, five-hour, and weekly evidence carry independent validity and quality. Serialized timestamps are RFC 3339 UTC, durations name milliseconds, and quota changes name percentage points on a 0–100 scale. Breaking shape or semantic changes require explicit major-version evolution; unsupported major versions are not silently accepted.

## Consequences

Rust and Python can generate types and validate shared fixtures against deterministic contracts. Strict objects catch typos and block arbitrary payload/metadata privacy bypasses, but additive fields require a schema revision and version-aware rollout. Tagged missingness is more verbose than nullable scalars but prevents unknown values becoming zero or defaults. Relative references work in a checkout and package, while deployment tooling must preserve schema layout. JSON remains human-readable and easy to inspect; runtime validation/tooling is deferred to P004.

## Alternatives considered

- **Unversioned JSON:** simple initially, but cannot safely negotiate semantic evolution.
- **Protocol Buffers:** strong generated types and evolution rules, but adds compilation/tooling before repository bootstrap and is less directly inspectable for research workflows.
- **MessagePack:** compact, but does not itself supply shared semantic validation and is less reviewable.
- **Database rows as the only contract:** couples analytics to storage migrations and cannot define normalized events or process results cleanly.
- **Ad-hoc Rust/Python structures:** invites drift, inconsistent missingness, and duplicated validation.

JSON Schema best fits the initial inspectable, language-neutral, local process boundary. These alternatives are not permanently prohibited; adoption would require evidence, an ADR, and preservation of these semantics.
