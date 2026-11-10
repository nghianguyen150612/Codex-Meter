# PROMPT039 implementation manifest

## Status and dependency

- P039 engineering status: **IN PROGRESS** until final-head cross-platform CI
  and release-bundle CI pass.
- Starting SHA: `58accdc8ac1e3226c555c04c0957415e2466386b`.
- Starting branch: `feat/p038-windows-execution-reliability`; this was the
  verified P038 head and current head of PR #33 at preflight.
- P038 PR #33 is open and Draft, targeting `main`, with verified head
  `58accdc8ac1e3226c555c04c0957415e2466386b`. Final-head CI passed in
  [PR run 42755734318](https://github.com/nghianguyen150612/Codex-Meter/actions/runs/42755734318)
  and release-candidate validation passed in
  [run 42755734292](https://github.com/nghianguyen150612/Codex-Meter/actions/runs/42755734292).
  Mandatory Windows live-runtime (G4) and physical Arch (G5) gates remain
  unverified. Neither physical gate is treated as passed.
- P039 branch: `feat/p039-task-token-attribution`.
- Required stacked PR base: `feat/p038-windows-execution-reliability` (PR #33).
- P039 PR #34: https://github.com/nghianguyen150612/Codex-Meter/pull/34
  (open, Draft; targets the P038 feature branch).
- P039 source head: `a4b5873b7b1508333ee366b68a3f5c283ec546e7`; final-head CI
  is running and remains pending.
- No migration is introduced. P039 reads existing P031 evidence using P032's
  `SqliteReadStore` and leaves P038 and P031–P038 history intact.

## Task identity and attribution

The validated persisted task key is `(source_id, session_id, task_id)`. All
three identities are required to attribute a response. The task ID is never
treated as globally unique by itself. A missing task or session identity stays
in a separate unattributed aggregate. Prompt text, timestamps, paths, model
names, session previews, and App Server associations do not supply identity.
App Server thread/turn IDs have no validated mapping to hashed rollout/session
identities in this feature.

Only checksummed `per_response` records contribute to canonical consumption.
Turn/thread cumulative and token-count snapshots are excluded. Source-reported
`raw_total` remains distinct from every component; counters are summed only
within their own source field. Missing counters remain missing and observed
zero remains zero. Cached input is not subtracted, reasoning output is not
added to output, and raw totals are never reconstructed. Checked arithmetic
returns an error on overflow. Duplicate source events are prevented by the
existing unique event ID ledger key and replay semantics.

Task summaries include response count, first and last timestamps, direct raw
and component totals with `MetricTotal` coverage, and distinct model/reasoning/
service-tier/Codex-version tuples. Missing configuration stays null. At most 64
configuration tuples are materialized per task and the summary marks
truncation. Tasks rank by known source raw totals and deterministic tie-breaks;
the query returns at most 1,000 records. The report includes all-scope
attributed and unattributed aggregates and global ingestion-quality diagnostics.

## Storage, CLI, and privacy

`SqliteReadStore` remains read-only, sets `PRAGMA query_only`, validates
migration history, and streams checksummed in-scope canonical rows without
loading the full history into memory. Result materialization is bounded by the
requested limit (maximum 1,000), and task rows are read in deterministic order.
No evidence is modified and no migration runs.

`codex-meter tasks` follows the UTC periods used by `tokens`, exact model and
session filters, and existing database path conventions. Human output contains
opaque task IDs, timestamps, response count, source raw total, bounded model /
reasoning summary, coverage status, and unattributed totals. JSON follows the
existing reporting schema version and states source, time bounds, filters,
observation status, page size, quality, and attribution contract.

No prompt, response, reasoning text, tool content, source path, repository
remote, credential, rollout JSON, or terminal transcript is read for this
report. No network service, upload, background daemon, App Server launch, or
task submission is involved. Raw local token counts are not official plan
allowances or account quota usage. Concurrent Codex Cloud, ChatGPT Work, and
other unobserved clients may not be represented.

## Validation and limitations

- Rust fixture coverage includes scoped task grouping, duplicate event IDs,
  multiple responses/models/tasks/sources, missing identity and counters,
  observed zero, out-of-order timestamps, snapshot exclusion, deterministic
  ordering, time/model/session filters, bounded top-task results, incomplete
  source coverage, checksum rejection, integer overflow, read-only byte
  preservation, migration-5 compatibility, and CLI JSON/human privacy checks.
- Local Rust format and all-target/all-feature Clippy: pass.
- Default workspace tests: pass (466 library, 4 analytics CLI, 19 sync CLI,
  5 TUI CLI, and 11 PTY recovery tests).
- All-feature workspace tests: pass (464 library, 2 analytics CLI, 19 sync
  CLI, 5 TUI CLI, and 11 PTY recovery tests).
- Python Ruff check and format: pass. Python tests: 666 passed. Contract
  validation: 6 schemas, 16 positive fixtures, and 5 negative fixtures passed.
- Product version policy: pass. P029 installed-runtime smoke: pass after
  building the local `codex-meter` executable.
- Linux local execution: passed for the Rust and Python suites.
- Windows, macOS Intel, and macOS ARM64: pending native CI for final P039
  source; not claimed from Linux-only local runs.
- Linux x86_64 release-artifact build, candidate packaging, and extracted
  candidate smoke: pass at implementation commit
  `6292ea016f3a1f1741b7506db1a15f43c9a0b656` (local candidate, not published).
- Final-head GitHub Actions and Windows/macOS native tests/bundles: pending.
- P038 G4/G5 remain unverified and are deferred to physical-device testing.
- The 0.157.1 rollout parser compatibility boundary remains unchanged.
- Task ranking can be limited by missing source raw totals; component counters
  are not used as a substitute ranking total.
- JSON configurations are explicitly marked truncated after 64 distinct tuples
  for one task.

Do not mark P039 engineering complete while a required check or final-head CI
is pending or failing. Integration remains blocked while PR #33's required
dependency gates remain unresolved.
