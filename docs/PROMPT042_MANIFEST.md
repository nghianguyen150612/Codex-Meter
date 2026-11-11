# P042 — Forecast readiness and evidence coverage

## Baseline and stack

Starting SHA: `88f1d4e48e11e638e957d241023916409d42f16d`, verified against
remote P041 and open Draft PR #36. Branch `feat/p042-forecast-readiness`
targets `feat/p041-workload-forecast-cli`. PRs #33 → #34 → #35 → #36 remain
unmerged; P038 G4/G5 remain **UNVERIFIED**, deferred by the user. No public
release or live Codex task is authorized or performed.

## Command and evidence boundary

`codex-meter readiness [--database PATH] [--request FILE] [--generated-at UTC_TIMESTAMP]
[--five-hour-used-percent N] [--weekly-used-percent N] [--json]`

Database defaults to the existing sync database location. Timestamp defaults to
current UTC; supplying it makes repeated reports deterministic. Python and
worker-mode options follow `workload` conventions. Missing databases produce an
`unavailable` inventory without creating storage. Invalid input and technical,
checksum, schema and worker failures remain categorized nonzero errors.

Inventory is informational. It reports checked P017 terminal candidates,
validity/quality counts, known configuration dimension counts, raw-total
presence, checked P031 canonical response counts, attribution coverage,
raw-total presence, quota snapshots and existing typed ingestion diagnostics.
It never calls a globally selected dataset ready. Unknown configuration values
remain unknown; no unbounded list of configurations or source records is emitted.

P031/P039 records establish directly observed response tokens and local opaque
task attribution. They do not establish terminal P017 Observation lifecycle,
quality, exact five-dimensional configuration, or task-specific quota deltas.
P039 tasks therefore cannot automatically become P022 samples. `sync` collects
audited P031 rollout evidence; it does **not** integrate complete qualified
P017 Observation generation. The next step may require a future milestone,
not a command that guarantees forecast availability.

Scoped readiness validates AnalyticsRequest v1 before database access. Each
requested meter is evaluated through P041's existing worker operation and Rust
semantic validation, with the same original request ID, configuration, range
and options, and a singleton subset of the original target meters. This keeps
missing current-position requirements independent between meters. No estimator
or eligibility algorithm is duplicated. If both meter reads return history,
the summaries must agree or the report fails closed. P041's command/result
contract remains unchanged. Its request execution is factored into a shared
internal function; the Python reader now enforces its existing 100,000-row
safety ceiling before materializing additional rows.

## Result states and guidance

The closed versioned `readiness-result.schema.json` describes aggregate output.
Inventory uses `selection_required`; absent databases use `unavailable`.
Scoped overall state is `ready` only when every selected meter is ready;
otherwise it is `insufficient_evidence` (or `selection_required` with no meters).
Per-meter `ready` is exactly P041/P022 `available`. Other P022 statuses remain
`insufficient_task_history`, `insufficient_capacity`, or
`insufficient_task_history_and_capacity`, with original reason codes and
P020 sample accounting/confidence. `unavailable` with
`current_quota_position_required` describes a blocked calculation; its capacity
confidence/accounting are null, not a fabricated insufficiency measurement.
Unsupported sources are reported separately from validated schema compatibility.
Unsupported-source warnings never create qualified evidence or override P022.

Fewer than three eligible task-size samples remain insufficient. Exact known
and unknown configurations follow the existing P017/P022 rules. Raw zero stays
available raw evidence but does not become a positive eligible historical size.
P020 capacity is an empirical raw-token-equivalent estimate, never an official
OpenAI allowance. Manual percentages, including zero, are unverified positions.
No stored account snapshot is substituted for a missing position. No current
quota is acquired. Source warnings, absent observations, missing raw totals,
missing snapshots and unattributed responses have explicit diagnostic codes.

Next-action codes mean: select an existing database; select configuration and
meters; supply a manual position when requested; or implement/integrate qualified
Observation evidence. They do not recommend invented grades, quota deltas,
unsupported Codex upgrades or changes to audited compatibility policy.

## Ownership, bounds and privacy

Rust reporting owns the new P031 inventory query. It scans at most 1,000 rows
per ledger table plus one lookahead in deterministic primary-key order, using
existing typed checksum readers. Cumulative snapshots never enter canonical
response counts. Counts use checked arithmetic; no token consumption is summed
or duplicated. The Rust scan has a 10-second interruption deadline.
Python's existing read-only P017 reader owns Observation inventory: at most
1,000 latest terminal rows, existing migration validation, JSON checksums and
projection integrity. The inventory reader also interrupts after at most
10 million SQLite VM instructions, rather than scanning an arbitrarily large
index or sort. At the limit the report conservatively marks coverage
partial, even when the database happens to contain exactly 1,000 candidates.
All inventory counts are **scanned coverage**, not guaranteed database-wide
totals. Each worker retains the existing bounded runtime/output supervision.

No migration, ingestion, network access, App Server, quota spending or evidence
mutation is introduced. Reports omit raw records, task/response identifiers,
prompts, responses, tool content, source code, paths, accounts and credentials.
Readiness configuration labels allow ASCII alphanumeric characters and `._+-`
only; unsafe labels fail with a sanitized error before evidence access.
Only selected safe configuration metadata and the opaque request ID are returned.
Failures use fixed sanitized messages. Reports are not persisted.

## Limitations and qualification

Inventory cannot determine configuration-specific eligibility. It reports
bounded coverage, not whole-database integrity certification. Separate domain
reads are not a single cross-language SQLite snapshot; changes between meter
history reads are rejected when summaries disagree. If P020 requires a missing
manual position, that meter has no finalized P041 history/capacity result; the
report explicitly shows it unavailable. No freshness or compatibility guarantee
beyond existing validated schemas and audited source policy is added.

Local and final-head CI evidence is recorded in the PR and delivery report.
Required checks cover Rust formatting/Clippy/both test modes, Python/Ruff,
eight schemas and positive/negative fixtures, installed-worker readiness and
inventory, Linux PTY, Windows ConPTY, both macOS architectures and four-platform
release candidates. Inherited G4/G5 remain unverified regardless of CI success.

P043 should narrowly integrate an explicitly evidenced P017 lifecycle collection
path, with real start/end and quality provenance and separately validated quota
samples. It must preserve unknown boundaries and never synthesize complete
Observations from task counts or account-wide quota causality assumptions.

## Delivery evidence

Draft PR: [#37](https://github.com/nghianguyen150612/Codex-Meter/pull/37).
Implementation commit: `a6d35dee1251e47287bc99dfa95581a27891136e`.
Local qualification of these source files passed:

- Rust format check and all-target/all-feature Clippy with warnings denied.
- Workspace/all-target tests: 532 passed; all-feature mode: 523 passed.
- Python: 677 passed; Ruff lint and format passed.
- Contracts: 8 schemas, 19 positive and 7 negative fixtures passed.
- Installed-worker smoke passed, including both readiness modes and read-only
  database checks; inherited Linux interactive controlling-terminal PTY passed.

An earlier concurrent local run observed the existing App Server malformed
transport fixture returning process-exit instead of malformed-response; complete
subsequent Rust runs passed. The installed-runtime smoke temporarily moves the
Python source tree for isolation testing and must complete before taking a
worktree snapshot or committing.

Final delivery qualification is recorded against the final branch HEAD in the
[PR checks](https://github.com/nghianguyen150612/Codex-Meter/pull/37/checks)
and PR description, including exact CI/release workflow run URLs and SHAs.
Cross-platform and release candidate success is not inferred from local tests
or from P041. This manifest's documentation-only delivery commit follows the
implementation commit; final-head workflow verification remains required.
