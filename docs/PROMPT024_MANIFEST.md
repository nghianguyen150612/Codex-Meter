# PROMPT024 Manifest

## Integration and Git workflow

- Starting `main` SHA: `9edf62a9ef30e490abd81af57f2f016300de92c9`.
- Verified `origin/main` after fetch at the same SHA; initial worktree was clean.
- Feature branch: `feat/p024-controlled-benchmark-framework`.
- Implementation changes are confined to the feature branch; no implementation was pushed directly to `main`.
- Required implementation commit: `3a9921e222f50ca3538b6da04ec9ba0205305706` (`feat: implement controlled benchmark framework`).
- PR title: `Add controlled benchmark experiment framework`.
- PR, review, checks, merge, and original P024 synchronization: complete.

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
- Local platform: Linux. Linux, macOS, and Windows Rust platform jobs passed in GitHub Actions.
- Review of changed-file scope and complete PR diff: complete; the PR contains only the 10 listed files.

## P025/P026 deferrals

- P025 controlled-runner/runtime integration, execution, and benchmark evidence capture are not implemented.
- P026 benchmark statistics, comparison, calibration, and reporting are not implemented.
- No live quota use or automatic retry was introduced.

## PR and merge record

- PR number and URL: [#16](https://github.com/nghianguyen150612/Codex-Meter/pull/16), `Add controlled benchmark experiment framework`.
- All six GitHub Actions jobs passed on the final PR head `a819c842247d9bdcaf76ec80d903b605f42485d7`: `rust-quality`, `rust-platforms (ubuntu-latest)`, `rust-platforms (macos-latest)`, `rust-platforms (windows-latest)`, `python-quality`, and `contracts`.
- Merge method: `merge` (Create a merge commit).
- Merge commit: `0f4a5e15ac452c00940cc62b6f2326ef90126d86`; parents, in order: `9edf62a9ef30e490abd81af57f2f016300de92c9` and `a819c842247d9bdcaf76ec80d903b605f42485d7`.
- Original feature commits `3a9921e222f50ca3538b6da04ec9ba0205305706`, `6708e1cd100e9dac1cae8b485d1885dc625c881d`, and `a819c842247d9bdcaf76ec80d903b605f42485d7` are ancestors of the merge commit and `main`.
- After PR #16 merge verification, local `main` equaled `origin/main` at `0f4a5e15ac452c00940cc62b6f2326ef90126d86`, and the worktree was clean.
- These post-merge facts were added through a documentation-only closeout PR because GitHub creates the merge SHA and parents only after merging.
