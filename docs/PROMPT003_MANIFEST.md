# Prompt 003 Manifest

## Identification and baseline

- **Prompt:** P003 — Versioned Domain and Data Contracts
- **Starting SHA:** `d3e125199e019957f2e0e3c165bc71e31d2671df`
- **Branch:** `work`
- **Verified P002 baseline:** `HEAD` exactly matched the expected merged P002 baseline. It is merge commit `d3e1251`, containing P002 documentation commit `303f168`; no newer commits or pre-existing worktree changes were present.
- **Final SHA:** Not embedded because a commit cannot contain its own SHA. The completion report records it.

All authoritative P001/P002 documents were read before editing. No architecture clarification was necessary, so existing P001/P002 documents remain unchanged.

## Files created

- `docs/DATA_CONTRACTS.md`
- `docs/adr/ADR-002-versioned-data-contracts.md`
- `docs/PROMPT003_MANIFEST.md`
- `schemas/v1/common.schema.json`
- `schemas/v1/normalized-event.schema.json`
- `schemas/v1/quota-sample.schema.json`
- `schemas/v1/observation.schema.json`
- `schemas/v1/analytics-request.schema.json`
- `schemas/v1/analytics-result.schema.json`
- Eight JSON fixtures under `fixtures/contracts/v1/`

## Files modified

None. P003 only creates the files listed above and does not modify unrelated work.

## Contract and versioning decisions

- Draft 2020-12, repository-relative references, and strict closed objects define six v1 schemas.
- Every top-level boundary object carries `schema_version: 1.0.0`; patch/minor/major evolution follows documented semantic compatibility rules, and unsupported major versions are rejected or negotiated.
- Tagged availability/provenance distinguishes observed, derived, and unavailable values, including known zero versus unavailable.
- RFC 3339 UTC timestamps, non-negative millisecond durations, IEEE-754-safe JSON integers, 0–100 percentages, and explicit percentage-point deltas make units interoperable.
- Configuration dimensions remain extensible strings with explicit unknown states.
- Raw token counters, plan quota, and future weighted/effective metrics are structurally separate.
- Observations represent the complete P002 lifecycle and independent token/five-hour/weekly validity and quality.
- Reset detection structurally forbids a quota delta for the crossed window.
- Analytics results identify empirical raw-token estimates, canonical P25/P50/P75 values, Codex Meter confidence, and transparent candidate/valid/excluded/outlier/used counts.

## Privacy decisions

Schemas use `additionalProperties: false` and narrow, enumerated metadata. They define no prompt, response, source code, repository, remote, email, account identity, credential, token, cookie, authorization header, arbitrary metadata, raw payload, path, or free-text diagnostic fields. IDs are opaque and synthetic fixtures are privacy-safe.

## Explicitly deferred work

P003 introduces no Cargo workspace, Rust crate/module, Python package, SQLite schema/migration/access, watcher, telemetry discovery/parser, quota client/provider, reconciliation or reset runtime, estimator algorithm, benchmark execution, task clustering, TUI, web dashboard, or community upload. P004 owns bootstrap and CI/schema tooling; later roadmap prompts own implementation and discovery.

## Validation performed

Validation covered JSON syntax, required files and versions, repository-local reference resolution, privacy-field checks, units/ranges, missingness, per-domain validity, reset semantics, sample accounting, raw/weighted separation, scope, whitespace, and the complete Git diff. No standards-compliant JSON Schema validator was installed, so fixture-to-schema semantic validation is explicitly deferred to P004 tooling; static schema assertions and manual review were performed. Exact commands and results are reported in the completion response.

## Known limitations and unknowns

Source locations/formats, quota acquisition, meter latency/reset evidence, configuration-field availability, shared concurrent quota behavior, estimator calculations, and runtime-to-storage mapping remain deliberately unknown. Schemas cannot establish cross-field arithmetic/time ordering by themselves; runtime validation must check used/remaining consistency, chronological ordering, count relationships, and estimator arithmetic. P004 should install repeatable Draft 2020-12 validation in CI and preserve relative schema layout when packaging.

## Unrelated work confirmation

The starting worktree was clean. Only P003 contract documentation, schemas, and synthetic fixtures were created; no unrelated or pre-existing work was modified.
