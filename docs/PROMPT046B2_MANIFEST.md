# P046B2: Durable controlled quota measurement sessions

## Scope and delivery

P046B2 adds a local bookkeeping workflow that binds two explicitly saved
P045B snapshots to one bounded user-managed interval. It reuses the P046B1
`evaluate_live_quota_comparison()` evaluator and preserves its independent
five-hour and weekly outcomes. It does not execute or observe the work between
the snapshots.

The intended sequence is:

```text
codex-meter quota --live --save
codex-meter quota measure start --before-snapshot lqs_... --confirm-account-scope
# User performs work independently.
codex-meter quota --live --save
codex-meter quota measure finish qms_... --after-snapshot lqs_... --activity unknown --confirm-account-scope
codex-meter quota measure status qms_...
```

The before and after acquisition timestamps bound the reported account-meter
comparison. Session start and completion timestamps record the user's local
workflow. The actual workload start/end is not independently verified.

## CLI and side effects

`quota measure start`, `finish`, `status`, and `abort` are explicit subcommands.
All accept `--database PATH`; output commands accept `--json`. Start requires
`--confirm-account-scope` and a saved snapshot ID. Finish requires the saved
after snapshot and confirmation; activity defaults to `unknown` and is limited
to `unknown`, `known_local_overlap`, and `self_reported_no_known_overlap`.
Duration defaults to 60 minutes and may be 1–240 minutes. The before snapshot
must be no more than five minutes old when the session starts.

Measurement commands do not acquire quota snapshots, call App Server, poll,
launch a task, or write task content. The user obtains both snapshots with the
separate `quota --live --save` command.

## Persistence and lifecycle

Migration 8 creates `controlled_quota_measurement_sessions`, referencing the
immutable saved snapshot IDs and checksums. Foreign keys are enabled for the
SQLite writer connection, and the schema prevents deletion of referenced
snapshots. A partial unique index allows only one `open` session per database.
Each explicit transition uses an immediate SQLite transaction and revision
compare-and-swap. Start stores a finite deadline. Finish validates the current
session, both checksum-valid snapshots, chronology, policy interval, and
deadline, evaluates P046B1, writes the terminal receipt, and commits atomically.

The terminal receipt checksum covers the session and snapshot IDs/checksums,
duration policy/version, activity declaration, completion time, terminal state,
fixed account-level provenance, and evaluator contract version `1.0.0`.
Completed receipts cannot be updated. Exact finish replay is idempotent;
conflicting finish requests fail. Aborted and expired sessions cannot be
completed. An overdue open session is reported as deadline-passed by read-only
status; an explicit abort closes it as expired. An attempted finish after its
deadline also persists the expired terminal state.

Status opens SQLite read-only, applies no migration, performs no RPC, and does
not mutate database bytes. Completed results are rebuilt from the two referenced
snapshots and the P046B1 evaluator when read. Five-hour and weekly evidence stay
independent; reset crossings, missing reset metadata, unavailable meters, and
unchanged percentages retain the P046B1 meanings.

## Trust and privacy limits

The activity declaration is a bounded user statement. Even
`self_reported_no_known_overlap` is not verified isolation. A session cannot
prove task causality, account identity continuity, inactivity on other devices,
complete token event capture, or production quota accuracy. Account-level
percentage-point movement is never called tokens consumed by the session's
work, official token capacity, or a stabilized task estimate.

The workflow does not produce P044 credentials or qualified events, P012 task
reconciliation, P013/P016 Observations, estimator inputs, quality grades, or
readiness promotions. It stores no task descriptions, prompts, repository
paths, account identifiers, credentials, raw RPC data, or free-text notes.

## Validation and remaining qualification

Implementation validation is recorded in the pull request and final report.
Deterministic fixture tests use file-backed SQLite and do not access a live
Codex account. Passing local tests does not establish authenticated source
behavior, production quota accuracy, account identity continuity, external
activity isolation, task-specific causality, token coverage closure, or P038
G4/G5 hardware qualification. Those gates remain unverified.

## RC1 fixture correction and validation (acceptance remains partial)

The CLI fixture used current acquisition times with fixed November 2026 resets.
Once the five-hour reset was past, the comparator correctly suppressed movement.
The fixture now captures a UTC reference for the before acquisition and both
reset identities; the after acquisition uses the returned session start time to
satisfy the actual CLI chronology. Resets are four hours and four days after
that reference. These are test values, not provider window assumptions. No
production comparison policy changed.

The file-backed expiry test now checks the committed expired revision after
reopen, absence of a receipt and after snapshot, identical and conflicting
finish rejection, and a replacement open session. It re-reads the expired row
and checks its state and revision. Further independent acceptance review is
needed for unrelated snapshot immutability and every requested invariant.

Local RC1 validation: the corrected CLI test passed once and three repeated
runs; six controlled-measurement tests passed. Rust formatting and Clippy
passed; all-feature tests passed (621 unit and 43 integration tests). Default
suite passed after installing the local Python worker (623 unit and 50
integration tests). Its initial run failed six analytics integration tests due
to worker resolution in the fresh environment. Python: 679 passed; Ruff lint
and format passed; contracts: 8 schemas, 19 positive and 7 negative fixtures.
P029 installed-runtime smoke passed. These results precede the final commit.

Historical PR CI run 44884388374 failed on macOS Intel in attempt 1 in
`compiled_task_history_is_readonly_resizes_and_never_submits_a_turn` with
`expected="Task usage" raw_contains=true screen_contains=false`; attempt 2
passed. The cause was not established. Final-head hosted validation and the
complete independent acceptance matrix remain pending; do not treat this
record as engineering acceptance. Production source qualification remains
unverified as described above.
