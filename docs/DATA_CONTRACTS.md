# Codex Meter v1 data contracts

## Authority and scope

This document is the human-readable semantic authority for the Draft 2020-12 JSON Schemas in `schemas/v1/`; the schemas are the machine-readable authority for shape and validation. Implementations must satisfy both. These contracts translate the product loop **Track → Measure → Estimate → Compare** while preserving:

```text
raw tokens != weighted/effective usage != plan quota
```

They specify normalized boundary data, not SQLite rows, source formats, runtime algorithms, or provider behavior.

## Versioning and compatibility

Every durable or process-boundary top-level object carries `schema_version`, initially `1.0.0`. Semantic Versioning applies to the contract:

- **patch** clarifies or corrects validation without intentionally changing accepted meaning;
- **minor** adds a backward-compatible semantic/schema extension;
- **major** changes shape or meaning incompatibly.

A consumer must reject or explicitly negotiate an unsupported major version; it must never silently interpret it as supported. Stable objects are closed with `additionalProperties: false`, so misspellings and privacy-bypassing fields fail validation. Consequently even an additive field requires a published schema revision and consumers that understand it; producers must not send it to older strict consumers without version negotiation. No generic extension bag is provided. Repository-relative `$ref` values are canonical within this repository; no external Codex Meter schema host is fabricated.

## Common primitives and units

- **Identifiers** are opaque strings of at most 128 safe characters. They are locally assigned correlation/idempotency handles, never floating-point numbers, prompts, paths, remotes, email addresses, account identifiers, or credentials. A `local_window_id` is explicitly local and is not claimed to be provider-issued.
- **Timestamps** are RFC 3339 UTC strings ending in `Z`. Local time is presentation-only. Runtime monotonic clocks may later measure elapsed time, but serialized wall-clock evidence is UTC.
- **Durations** are non-negative integer `duration_ms` values.
- **Observed counters** are non-negative JSON integers no greater than `9,007,199,254,740,991`, the IEEE-754 exact-integer ceiling. Rust must range-check before serialization/deserialization; Python must preserve integers and enforce the schema bound. This makes interchange safe even through ordinary JSON implementations backed by binary64 numbers.
- **Estimated token quantities** are non-negative JSON numbers because statistical estimates and percentile values may be fractional.
- **Quota values** are percentage points on the inclusive 0–100 scale. Thus 20 to 40 is a 20 percentage-point change, never a ratio of 0.20. `delta_percentage_points` is non-negative consumption within one continuous window.

## Availability and provenance

Values whose absence matters use tagged structures rather than `null` or a guessed default. An available value has `availability: available`, a `value`, and provenance `observed` or `derived`; unavailable values have `availability: unavailable` and provenance `unavailable`, with no value. Therefore known zero differs from unavailable. `observed` means directly normalized from allowed source evidence, `derived` means locally computed from identified evidence, and `unavailable` means no defensible value exists.

Configuration identity contains plan, model, reasoning level, speed mode, and Codex version. Each dimension independently uses this representation and accepts future non-empty names instead of a closed list. Unknown speed is not standard; unknown reasoning tokens are not zero. Dataset selectors preserve every dimension, preventing silent pooling of configurations such as `plus / gpt-5.6-sol / high / standard` and `plus / gpt-5.6-sol / high / fast`. These fields describe segmentation evidence and assert no provider metering rule.

## Raw token telemetry

`tokenCounters` has separate `uncached_input`, `cached_input`, `output`, `reasoning_output`, and `raw_total` metrics. Every metric is explicitly available or unavailable. A directly observed total and a locally derived total differ by provenance. `raw_total` has no schema-imposed summation identity because source semantics remain undiscovered. Weighted/effective usage has no v1 counter field and must never be placed in a raw-token field; future derived metrics require a versioned, explicitly named contract and formula provenance.

## Quota samples and windows

A quota sample records one `five_hour` or `weekly` meter at a UTC instant. When reset/window evidence is available, its meter type must equal the sample’s top-level meter type; unavailable reset evidence remains valid without a fabricated identity. Used and remaining percentages are independently available: neither is required to be available merely because the other is, and deriving one must be labeled `derived`. Acquisition status and a narrow sanitized source kind preserve operational provenance without a raw payload. Configuration context is retained with explicit unknowns.

A quota-window identity is either explicitly unavailable or supported by local evidence. Available identities include the meter type, observed reset and/or locally generated opaque identity, evidence sample bounds, provenance (`observed_reset` or `locally_inferred`), and confidence. This representation does not presume a provider window ID. Confidence is Codex Meter's assessment, not provider confidence.

## Normalized runtime events

`normalized-event.schema.json` covers five minimal families: session detection/start/end, token updates, and configuration evidence. Events carry opaque event/source/session/task/cursor identities and UTC event time for future idempotency. The event discriminator and payload discriminator are coupled: session detection/start/end require a lifecycle payload, token updates require a token-counter payload, and configuration evidence requires a configuration payload. Discriminated payloads contain only lifecycle, normalized counters, or configuration. They deliberately exclude provider field names, filenames, source payloads, and content. Source adapters must privacy-filter before producing these events.

## Observations, lifecycle, validity, and quality

An observation's lifecycle is one of `detected`, `active`, `task_ended`, `awaiting_meter`, `reconciling`, `finalized`, `incomplete`, or `invalid`. `task_ended` is deliberately distinct from `finalized`. Timing fields are optional when unknown and use explicit units.

Evidence validity is independent for tokens, five-hour quota, and weekly quota:

- `valid`: normally eligible for the applicable estimator;
- `incomplete`: normally excluded or explicitly downgraded by later estimator policy;
- `invalid`: excluded from that domain's estimator;
- `unavailable`: no evidence exists for that domain.

The schema does not implement weighting policy. Quality grades retain product semantics: `A` controlled benchmark, `B` isolated normal task, `C` possible concurrent usage, `D` delayed/incomplete meter evidence, and `X` invalid. An observation summary is useful for display, but domain-specific validity and quality govern quota estimation. Suggested numerical weights remain estimator policy and are not serialized.

Compact reason codes explain degraded evidence without free-text content: quota reset crossed, meter unavailable/unstable, telemetry incomplete, concurrent usage possible, process interrupted, source acquisition failure, and unknown reason.

Each named quota branch locks its meter type (`five_hour` or `weekly`), and an available window identity must carry that same meter type. Each quota domain contains before/after snapshots, window identity, reset status, validity/quality/reasons, and—only when defensible—a non-negative `delta_percentage_points`. A detected reset structurally forbids a delta; valid quota evidence requires one. Thus a 97-to-4 reset crossing is invalid for that window rather than valid `-93`, while token and the other quota window can remain valid.

## Analytics process contracts

`analytics-request.schema.json` is the semantic Rust-to-Python request. It correlates an opaque request ID with a stable analysis type, full configuration selector, optional UTC history range, target quota windows, and narrow estimator options. It contains no process command, SQLite path, or content.

`analytics-result.schema.json` is the Python-to-Rust result. It correlates the request and analysis type, records status, configuration, generation time, estimator method version, warnings, and per-window capacity output. Its metric discriminator is `estimated_raw_tokens`, and `provider_authority` is fixed to `codex_meter_empirical_estimate`: results cannot masquerade as an official allowance or weighted/effective telemetry. Successful-confidence estimates carry explicitly raw-token-labeled full/used/remaining quantities and canonical `p25`, `p50`, and `p75` capacity percentiles; `p50` is the sole machine representation of the median.

Confidence is `insufficient`, `low`, `medium`, or `high`, always Codex Meter's statistical assessment. Exact computation is deferred. Each estimate separately reports candidate observations, valid observations, validity/quality exclusions, statistical outliers removed, and observations actually used. An insufficient result may omit numerical estimates while retaining accounting and reasons.

## Privacy by schema

Closed objects and narrow fields intentionally provide no place for prompt/response text, source code, repository contents, Git remotes, user email, account IDs, credentials, OAuth/access tokens, cookies, authorization headers, arbitrary metadata, or raw telemetry payloads. Logs, persistence mappings, and future diagnostics must preserve the same restriction. Synthetic fixtures contain only opaque IDs and numerical evidence.

## Evolution rules

Changes begin in this document and corresponding schemas/fixtures together. Compatible additions receive a minor contract version and require explicit producer/consumer negotiation because objects are strict. Validation corrections that preserve intended meaning use patch versions. Renames, removals, changed units/meaning, relaxed privacy boundaries, or incompatible required fields require a major-version directory/contract evolution. Old major versions may coexist during migration. Alternative encodings remain possible later only through an explicit ADR and equivalent semantics; database rows are never the sole cross-language semantic authority.
