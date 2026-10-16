# PROMPT025 manifest

## Repository and workflow

- Starting `main` SHA: `3a632d12dfd013b454719d8a289b246866d6082e`.
- Required feature branch: `feat/p025-authorized-benchmark-runtime`.
- Starting SHA and clean worktree were verified before implementation.
- Workflow: fetched `origin/main`, created the required feature branch, and kept
  implementation changes off `main`. Commit, push, pull request, CI, review,
  and merge verification are recorded after those steps complete.
- Required PR title: `Integrate authorized benchmark runtime and evidence capture`.
- Required implementation commit subject:
  `feat: integrate authorized benchmark runtime`.

## Runtime design

- Runtime version: `BENCHMARK_RUNTIME_VERSION = 1.0.0`.
- Adapter architecture: `BenchmarkRuntimeCoordinator` uses typed evidence,
  authorization, workload resolution, execution, dispatch, and Observation
  resolver contracts. The filesystem journal is caller-selected and Rust-owned.
- Supported production execution capability: `Disabled` only.
- Simulation: test-only fake adapters and synthetic evidence; never uses
  provider credentials, provider network, Codex CLI, quota, or production
  Observation writes.
- Live execution: unavailable. No production provider adapter, trusted
  user-action ingress, or production preflight evidence source exists.
- Authorization: a sealed trusted one-run action is scoped to experiment, run,
  repetition, and one invocation. The public P024 authorization value alone is
  not an action proof. No global permission or batch expansion is provided.
- Preflight gates: configuration; telemetry source; each requested quota
  meter's availability and window continuity; isolation; execution boundary.
  Unknown stays unknown, exact per-meter coverage is required, and verified
  mismatches block readiness.
- Journal: version `1.0.0`; atomic create-if-absent authorization index and
  reservation, enforcing one scope per authorization ID across restarts;
  strict bounded records, file sync before invocation, separate acknowledgment
  and terminal markers, fail-closed corruption and I/O behavior, no workload
  payloads.
- Concurrency: journal lock serializes reservation and claim ownership; an
  atomic active-dispatch lock prevents overlapping runs across threads,
  coordinators, and processes. Stale locks block dispatch for operator review.
- Recovery: reserved without acknowledgment is uncertain. Recovery reads
  state only, never dispatches, regenerates authorization, retries, or deletes
  stale records.
- Observation linkage: only an existing SQLite Observation can be resolved.
  Crate-issued task/session/source correlation is checked against its actual
  identity before P024's private trusted binding is used.
- Shared claims: one process-wide P024 claim ledger plus durable journal claims
  preserve one owner per Observation across active experiments and restarts;
  cross-process participants must share one journal directory.
- Isolation and quality: unknown isolation remains unknown; benchmark labels,
  authorization, preflight, workload match, and process success never assign
  Grade A or controlled isolation.
- Per-meter evidence remains independent; P025 does not calculate quota deltas
  or change Observation quality.

## Scope and compatibility

- Dependencies added: none.
- SQLite migrations or schemas changed: none.
- Python write path or runtime dependencies changed: none.
- CLI, scheduler, quota calculations, benchmark statistics, and P026 work:
  none.
- Privacy: bounded opaque IDs, status, and UTC times only; no prompts,
  responses, commands, credentials, environment variables, reasoning, or source
  content are persisted or logged.
- P026 deferrals: controlled benchmark comparison, calibration analysis,
  statistical evidence reporting, and estimator-quality assessment.

## Verification

- Runtime-specific tests: passed (96 benchmark tests, including P024 lifecycle
  tests and P025 dispatch/runtime tests).
- Python Ruff check and format: passed.
- Python tests: passed (438 tests).
- Rust fmt and clippy: passed; full workspace tests: passed (254 tests);
  workspace build: passed.
- Contract validation: passed (6 schemas, 16 positive fixtures, 5 negative
  fixtures).
- `git diff --check`: passed. Published migration and schema paths are
  unchanged; the four SQL migrations remain `i/lf`, `w/lf`, with `eol=lf`.
- `just check`: unavailable because `just` is not installed.
- Actual live provider workload launched: no.
- PR #18: [Integrate authorized benchmark runtime and evidence capture](https://github.com/nghianguyen150612/Codex-Meter/pull/18).
- Full PR diff review: the GitHub diff matched the reviewed local diff exactly
  (10 files); no scope, privacy, schema, migration, or provider-execution
  blocker was found.
- CI run `38850898587` on feature commit
  `760dcaf0c9224f15b20b057b395893ddc6c2daae`: all six jobs passed (Rust
  quality, Ubuntu/macOS/Windows Rust platforms, Python quality, contracts).
- At review, GitHub reported the PR mergeable with a clean merge state and no
  outstanding review decision. The branch-protection detail endpoint was not
  accessible to the current GitHub token; GitHub will enforce any protection
  rules when the authorized merge is attempted.
- Merge method: merge commit is required. Actual merge SHA, both parents,
  feature-commit reachability, final main equality, and clean-worktree results
  are reported in the completion response; no future merge SHA is predicted.
