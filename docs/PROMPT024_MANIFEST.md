# PROMPT024 Manifest

## Integration and Git workflow

- Starting `main` SHA: `9edf62a9ef30e490abd81af57f2f016300de92c9`.
- Verified `origin/main` after fetch at the same SHA; initial worktree was clean.
- Feature branch: `feat/p024-controlled-benchmark-framework`.
- Implementation changes are confined to the feature branch; no implementation was pushed directly to `main`.
- Required implementation commit: pending commit.
- PR title: `Add controlled benchmark experiment framework`.
- PR, review, checks, merge, and final synchronization: pending.

## Framework and API

- Framework version: `1.0.0` (`BENCHMARK_FRAMEWORK_VERSION`).
- Rust modules: `benchmark::{model, validation, lifecycle, evidence}` exported from `lib.rs`.
- Public API includes validated `BenchmarkPlan`, distinct experiment/run/workload/authorization IDs, `BenchmarkPreflight`, `BenchmarkExperiment`, `BenchmarkRun`, explicit one-run authorization, trusted Observation-link types, and a shared in-memory Observation claim ledger.
- State is an in-memory deterministic model. Only the plan has a versioned JSON representation in P024; lifecycle state is not a published subprocess or persistence protocol.

## Plan and preflight rules

- Plans retain the existing `ConfigurationIdentity`, workload key, distinct experiment ID, positive run bound (maximum 100), unique supported quota meters, framework version, and explicit UTC creation time.
- Opaque IDs are bounded safe identifier tokens. Configuration availability and provenance are preserved; unknown values remain unknown.
- Plan deserialization rejects invalid identities/configuration/timestamps/counts/enums and unknown fields, including payload-like fields.
- Preflight uses typed evidence for configuration, telemetry source, each selected meter, each selected meter’s window continuity, isolation, and execution-boundary readiness. Each snapshot is bound to the exact plan and cannot be reused for a different configuration or workload.
- `ready` requires every gate to have verified evidence. Missing gates remain unknown; explicit blockers produce blocked preflight.
- Preflight readiness is not user permission. Each invocation needs a separate authorization event bound to one experiment, run, and repetition.

## Lifecycle and Observation contract

- Lifecycle: planned → preflight pending → ready/blocked/unknown → authorized → running → awaiting Observation → completed/incomplete; abort is terminal from nonterminal stages.
- Explicit UTC timestamps are transition inputs. Exact replay is unchanged success; conflicting replay is rejected.
- Distinct run IDs and repetition indices are enforced, and runs cannot exceed the plan count. A later run does not mutate earlier runs.
- Linkage requires a crate-issued trusted runtime binding, a validated existing Observation identity, terminal lifecycle plus finalization evidence, exact configuration, temporal consistency, and no prior claim by another run through the shared claim ledger.
- Nonterminal or unverified Observation linkage cannot complete a run. Observation fields and quality are never changed by benchmark association.
- Five-hour and weekly evidence remain separate in the authoritative Observation. The benchmark model stores no duplicate quota reconciliation, raw token total, or quota delta.
- Isolation is independent from the benchmark label. Unknown/possible concurrent use remains explicit; P024 never assigns `QualityGrade::A` or `IsolationEvidence::ControlledBenchmark`.

## Privacy, dependencies, and compatibility

- No prompt, response, reasoning trace, tool output, source code, paths, Git remotes, user email, tokens, credentials, or generic metadata dictionary is accepted.
- No provider execution, network client, subprocess, process discovery, CLI command, background worker, GUI/TUI, SQLite write, or Python write path was added.
- No Rust dependency or Python runtime dependency was added.
- No migration, schema registry, checkpoint format, or `schemas/v1/*` file changed.
- Existing P017–P023 behavior and raw-token/effective-usage/plan-quota distinctions were not changed.

## Validation results

- Rust tests: 198 passed (158 baseline plus 40 benchmark regressions).
- Python tests: 438 passed.
- Ruff lint and format: passed.
- Rust format, Clippy with warnings denied, tests, and workspace build: passed.
- Contract validation: 6 schemas, 16 positive fixtures, and 5 negative fixtures passed.
- Existing SQLite migration checksum regressions: passed; no migration was added.
- `git diff --check`: passed.
- Local platform: Linux. macOS and Windows results are pending the required GitHub Actions matrix.
- Review of changed-file scope and local diff: complete; the PR diff review remains pending until publication.

## P025/P026 deferrals

- P025 controlled-runner/runtime integration, execution, and benchmark evidence capture are not implemented.
- P026 benchmark statistics, comparison, calibration, and reporting are not implemented.
- No live quota use or automatic retry was introduced.

## PR and merge record

- PR number and URL: pending.
- Latest feature SHA reviewed: pending.
- Six GitHub Actions jobs: pending.
- Merge method: pending; required method is `merge` (Create a merge commit).
- Merge commit and its two parents: pending.
- Original feature-commit reachability: pending.
- Final `main` SHA and `origin/main` equality: pending.
- Final worktree status: pending.
