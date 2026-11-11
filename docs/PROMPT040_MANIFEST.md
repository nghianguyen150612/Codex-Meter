# PROMPT040 implementation manifest

## Scope and dependency

P040 makes P039 task attribution accessible in the existing History screen. The six
screens remain Overview, Sessions, Projects, Models, History, and Composer.

- Starting SHA: `24a5e1fef08a664a79d0a449fd25d5c328617ec3`.
- Branch: `feat/p040-task-usage-tui`.
- Required Draft PR base: `feat/p039-task-token-attribution` (PR #34).
- Preserved ancestry: main → P038 (#33) → P039 (#34) → P040.
- Preflight verified #33/#34 open, Draft, unmerged, at their supplied heads.
  P039 current-head [CI 42926916344](https://github.com/nghianguyen150612/Codex-Meter/actions/runs/42926916344)
  and [release validation 42926932598](https://github.com/nghianguyen150612/Codex-Meter/actions/runs/42926932598)
  completed successfully on the starting SHA. Older dependency manifests retain their historical
  pending statements; live run metadata was checked before implementation.
- Engineering qualification requires all required CI and release checks on the current P040 head.
  A pending/failed run means engineering is PARTIAL, even when local checks pass.
- Integration/release/merge remain **BLOCKED** by inherited P038 G4/G5 physical-device gates.
  Physical Windows live-runtime and Arch qualification remain **UNVERIFIED**, deferred by the user.
  No public release, merge, live task, or allowlist change is authorized by this milestone.

## Navigation and rendering

`5`, then `t` opens Task usage. `t` cycles Token events → Task usage → Quota observations.
Arrows/j/k select; n/p, PageDown/PageUp, and brackets navigate 50-row local pages.
The query loads **top 200 tasks** ranked by P039's direct raw-total ordering. `has_more`
shows TRUNCATED; local paging never retrieves unseen database rows or invents a cursor.
The CLI's `tasks --all --limit 1000` supports a larger bounded report.

`w` cycles the existing UTC All/Today/Week scopes, `m` applies an exact model filter,
`/` searches loaded safe metadata, and `r` refreshes. Filter changes clear task caches
and invalidate old generations, including changes made in another History mode.
Refresh preserves selection only by the full `(source_id, session_id, task_id)` key;
otherwise it chooses the first matching loaded row. Task selection is separate from
token/quota selection, so off-mode responses cannot move another view's selection.
Compact display IDs are SHA-256 prefixes over the length-prefixed composite tuple,
never lookup keys or inferred App Server associations.

Enter opens a bounded read-only snapshot of `TaskUsageSummary`. It displays complete
opaque identities, timestamps, response count, raw/input/cached/cache-write/output/
reasoning counters and known/missing coverage, configuration tuples, source quality,
and result/configuration truncation. Arrows/j/k scroll, n/p/PageUp/PageDown move ten
lines, Home/End select scroll bounds, and Esc/Enter close. Resize recalculates
Unicode display-column wrapping and clamps scrolling. Dismissal remains available in
the tiny-terminal fallback. Wide tables have six columns; narrow tables use four or
six lines per row. Too-small layouts request resize rather than hiding coverage.

## Accounting, loading, and privacy

- The existing `SqliteReadStore::task_usage` API owns all aggregation. No second SQL
  implementation, migration, new format, or ingestion pipeline is introduced.
- Only validated, checksummed `per_response` evidence contributes. Snapshots are
  excluded; raw totals are not reconstructed. Cached input is not subtracted;
  reasoning is not added to output. P039 checked arithmetic remains authoritative.
- Missing counters display Unavailable; partial counters retain known totals and
  missing coverage; directly observed zero stays zero. Unattributed response/raw
  totals remain separate and are never silently assigned to a task.
- Global diagnostics show unsupported/compressed sources, malformed evidence,
  continuity ambiguity, and partial collection. The overlay includes total global
  diagnostic count. These are collection warnings, not task-specific quota estimates.
- Local token usage is independent of account-wide five-hour and weekly quota
  snapshots. No quota percentage, billing estimate, or official capacity is inferred.
- Typed requests use the existing 8-command database queue and shared 32-result queue.
  Request IDs reject stale results. One existing database worker performs reads;
  refresh does not create threads or perform SQLite work on the rendering thread.
- A SQLite progress handler checks shutdown every 1,000 VM operations during task
  reporting. Backpressured reporting result delivery checks shutdown every 10 ms.
  Existing database lock timeout/schema validation and bounded App Server operations
  remain in place. No periodic refresh, background network call, or daemon is added.
- Loading and sanitized error states replace the table, even when cached results
  exist. A missing database points to `codex-meter sync`; unavailable/corrupt evidence
  fails closed with the existing sanitized error code. Empty scope and only-unattributed
  evidence are distinct; neither implies zero account usage. Queue/worker failure
  reports an actionable retry message rather than an empty table.
- Viewing tasks opens SQLite read-only/query-only and never mutates evidence,
  creates a missing ledger, executes/resumes a task, or calls App Server. Existing
  explicit sync and Composer flows retain their own confirmation boundaries.
- Only normalized typed metadata is presented. Prompt/response/reasoning content,
  tools, source code, source paths, remotes, credentials, emails, raw records, and
  transcripts are excluded. Path/URL/email/key-shaped configuration labels are hidden.
  Execution output and Composer text cannot enter task presentation.

## Deterministic validation

Fifteen new library tests reuse the P039 synthetic checksummed fixtures and Ratatui
TestBackend patterns. They cover all modes/screens, paging/search, exact composite
selection and fallback, stale generations, off-mode results, refresh/scope/model
changes, detail snapshot isolation, scroll bounds, unavailable/partial/zero counters,
multiple sources/models, missing identity, checksum corruption, top-list truncation,
source diagnostics, worker independence/cancellation/backpressure, read-only bytes,
Unicode/long labels, tiny layouts, resize, and privacy. Existing Composer, approval,
interruption, journal, and no-replay assertions remain intact.

One new compiled Unix PTY test ingests existing synthetic rollout fixtures, opens
History/task details, resizes the list and overlay, refreshes, exits, verifies terminal
restoration and unchanged database bytes, and requires zero execution/resume calls.
The existing startup metadata query reads both archive scopes; task navigation adds
no App Server query. Existing Windows synthetic ConPTY tests remain required.

Local managed Linux x86_64 validation (Rust 1.99.1):

| Check | Result |
| --- | --- |
| Rust format / all-target/all-feature Clippy | PASS |
| Default workspace | 481 library, 4 analytics CLI, 19 sync CLI, 5 TUI CLI, 12 Unix PTY PASS |
| All-features workspace | 479 library, 2 analytics CLI, 19 sync CLI, 5 TUI CLI, 12 Unix PTY PASS |
| Focused TUI library suite | 67 PASS (included in workspace totals) |
| Python / Ruff | 666 tests PASS; lint and 35-file format check PASS |
| Contracts | 6 schemas, 16 positive and 5 negative fixtures PASS |
| Product version policy / installed-runtime smoke | Required; recorded in current-head checks |
| Native Linux/Windows/macOS Intel/macOS ARM64 / bundles | Required; recorded in current-head checks |

Current-head evidence is tracked in the P040 Draft PR checks and
[branch Actions history](https://github.com/nghianguyen150612/Codex-Meter/actions?query=branch%3Afeat%2Fp040-task-usage-tui).
The CI workflow runs default/all-feature native suites, Linux interactive-parent PTY
reproduction, Python/contract checks, and all nine OS/Python installed-runtime cells.
Release candidate validation builds/reproducibly packages/extracts/smokes Linux x86_64,
Windows x86_64, macOS Intel, and macOS ARM64, then verifies aggregate provenance.
It does not publish a public release. Final reports must identify the exact tested
commit and successful workflow URLs; historical dependency success is not P040 evidence.

## Compatibility and remaining gates

The telemetry source boundary remains audited Codex 0.157.1. The execution allowlist
remains audited 0.163.0; 0.164.0 support is not added. Task reporting works without an
App Server installation or live account. No physical-device or live-account test is
required for P040 cloud engineering. G4/G5 remain distinct release blockers and all
three stacked PRs must remain Draft and unmerged while inherited gates are blocked.
No P041 feature, forecasting, cloud sync, new prompt feature, or public publishing is
included. See [TUI guide](V02_TUI_DASHBOARD.md), [reporting CLI](V02_USAGE_HISTORY_CLI.md),
and [compatibility matrix](V02_CODEX_COMPATIBILITY_MATRIX.md).
