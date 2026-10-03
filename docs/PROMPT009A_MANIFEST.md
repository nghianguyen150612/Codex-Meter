# Prompt 009A Manifest

## Identification

- **Prompt:** P009A — Preserve Late Root Task Evidence
- **Starting SHA:** `632242ab4ac9230c67b148414a99c3bebb06db01`
- **Branch:** `codex/p009-session-task-assembly`
- **PR:** #8, updated in place; P010 was not started.

## Review bug and correction

P009 originally assigned `root_task_id` only while inserting a task. A task first observed through token usage could therefore lose later root evidence from `task_started` or `turn_context`.

`TaskState` now retains the selected `root_task_id` plus an `observed_root_task_ids: BTreeSet<String>`. The centralized get-or-create helper applies every newly supplied root evidence: unknown lineage is backfilled, agreeing evidence is idempotent, and distinct roots are retained with the typed `ConflictingRootTaskEvidence` anomaly. The first known root remains selected; no root is silently overwritten.

Terminal and token processing do not invent root evidence and preserve already-known lineage.

## Files and tests

- Modified `rust/crates/codex-meter/src/telemetry/assembly.rs`.
- Modified `docs/SESSION_TASK_ASSEMBLY.md`.
- Added regression tests for task-start backfill, turn-context backfill, agreeing roots, conflicting roots, deterministic replay, and terminal preservation.
- Existing P008 deterministic-ID regression tests remain unchanged and passing.
- No schemas, dependencies, persistence, quota logic, or normalized event contracts changed.

## Validation and final SHA

Validation covers Cargo formatting, Clippy, workspace tests/build, Ruff, Pytest, contract validation, and `git diff --check`. The corrective commit is `fix: preserve late root task evidence`; its final SHA, push result, and PR update result are reported after commit operations.
