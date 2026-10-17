# PROMPT027 Manifest — CLI Analytics Integration

## Starting point and workflow

- Starting `main` SHA: `0cdbac51ab7d7ba3e87acf45c491020dc8d96a1c`.
- Required feature branch: `feat/p027-cli-analytics-integration`.
- Required implementation commit subject: `feat: integrate CLI capacity analytics`.
- Required PR title: `Integrate CLI with Python capacity analytics`.
- PR targets `main`; the implementation was kept off `main` and follows `docs/DEVELOPMENT.md`.
- Implementation commit: `74e47d4b0bf07e55e00ab63bcd3b396968fb9121`.
- PR #20: [Integrate CLI with Python capacity analytics](https://github.com/nghianguyen150612/Codex-Meter/pull/20), targeting `main`.
- Latest-head CI results, merge commit SHA and parents, feature-commit reachability, final `main` SHA, and final clean-tree verification are reported after remote CI and merge verification. No future identifiers are prefilled.

## Implementation

- CLI: `codex-meter --help`, `--version`, `estimate --help`, and `estimate --database PATH --request FILE --generated-at UTC_TIMESTAMP [--five-hour-used-percent N] [--weekly-used-percent N] [--python PATH] [--json]`.
- Rust modules: `cli` owns parsing/presentation; `analytics::request` and `analytics::result` own strict typed contract checks; `analytics::process` owns safe invocation; Python retains estimator algorithms.
- The native argument parser uses the Rust standard library; no new Rust runtime, Python runtime, or provider dependency was added.
- Python worker: `python -m codex_meter.integration.worker`; callable API `execute_request(raw, LocalExecutionContext)`.
- Local process protocol: `ANALYTICS_PROCESS_PROTOCOL_VERSION = 1.0.0`. Request JSON remains AnalyticsRequest v1; database path, timestamp, and manual meter percentages remain process arguments.
- Supported analysis type: `current_capacity_estimate` only. Other declared analysis types fail explicitly.
- Supported options: `maximum_observations` from 1 to 100,000. `minimum_quality` is rejected because P018's A/B/C eligibility accounting is authoritative.
- Current meter values are manually supplied percentage points, finite and within `[0, 100]`; result JSON does not invent source provenance. Human output labels them manual and unverified.
- The selector maps the five configuration values/provenance exactly and passes history bounds and maximum rows to P017. No configuration values are inferred and no ranges are broadened.
- P017 uses the existing strict read-only SQLite reader, verifies migration checksums and Observation integrity, enables query-only behavior, and does not migrate or create a database.
- Rust validates result schema, request identity/scope, configuration, target meters, authority/method, confidence/status, finite estimates, sample accounting, arithmetic, percentiles, reason codes, warnings, and unknown fields.
- Process limits: 64 KiB request, 2 MiB result, 8 KiB retained diagnostics, 60-second timeout, concurrent pipe draining, child termination and reaping.
- Human output includes raw-token units, empirical confidence/sample accounting/reasons, manual-position labeling, and the non-official-allowance footer. JSON output contains only validated AnalyticsResult v1.

## Validation, privacy, and limits

- Local quality gate passed: Ruff check/format, 615 Python tests, Rust format/Clippy, 273 Rust unit tests plus 3 real subprocess integration tests, Rust build, 6-schema/16-positive/5-negative contract validation, and `git diff --check`.
- The real CLI integration tests use a synthetic database created through Rust `SqliteStore`, compare database bytes before/after analytics, verify zero-evidence output, and verify configuration filtering without pooling.
- The six GitHub Actions results, including Ubuntu/macOS/Windows subprocess execution, are checked against the latest PR head before merge.
- Database is never opened for analytics through Rust `SqliteStore`; tests compare the synthetic database before and after worker execution.
- No provider APIs, authentication, live benchmarks, workload execution, prompt/source content, or persisted analysis results are introduced.
- Python runtime dependencies remain unchanged. No public schema or migration files are changed.
- P028 deferrals: privacy verification, cross-boundary failure isolation, and integration security hardening. P029/P030, automatic quota acquisition, live benchmark execution, and release packaging remain untouched.

## PR and merge verification

The feature branch and PR metadata are populated after PR creation. Merge SHA and parent/reachability/final-main facts are verified after merge and returned in the completion report, as required by the workflow. P027 does not begin P028.
