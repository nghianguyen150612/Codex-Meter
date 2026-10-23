# Execution recovery journal (P037)

P037 adds a write-ahead journal around the P036 controller. Codex Meter records only local
operational identities and state. Codex App Server remains the source for thread and turn status;
the journal is not a usage ledger and does not prove that a provider request was sent.

## Storage and privacy

On Linux, the journal directory is `$XDG_DATA_HOME/codex-meter/recovery` when `XDG_DATA_HOME` is
absolute, otherwise `~/.local/share/codex-meter/recovery`. On macOS it is
`~/Library/Application Support/codex-meter/recovery`. On Windows it is
`%LOCALAPPDATA%\codex-meter\recovery`. Execution fails closed if the platform backend cannot
verify the private location and current-user ownership.

The directory contains a single bounded `journal.json`, an advisory `.lock`, and at most one
`.journal.tmp` replacement file. Unix directories are opened by components with no-follow checks;
the journal directory is owned by the current user and mode `0700`, and journal, lock, and temporary
files are owned by that user and mode `0600`. Windows uses a protected DACL on the journal
directory, lock, journal, and replacement file. The DACL grants full access only to the current
user SID; the backend refuses non-local or non-fixed drives, reparse points, non-regular files,
different owners, inherited or additional ACEs on private entries, and paths it cannot validate.
Both backends preserve malformed or corrupt data instead of silently replacing it.

Unix updates serialize through a process-local mutex and OS `flock`, compare the record revision
before writing, write a fixed temporary file, `fsync` it, atomically rename it over the journal, then
`fsync` the containing directory. Newly created directory entries are also synchronized in their
parent. Windows writers flush the same-directory temporary file with `FlushFileBuffers`, use
`ReplaceFileW` for replacement or `MoveFileExW` with write-through for first creation, and
coordinate with `LockFileEx`; acquisition is bounded to five seconds. The Windows kernel releases
byte-range locks when the owning handle or process closes. A process-local mutex covers threads in
the same process. A stale writer receives a revision conflict instead of overwriting newer state.

## Version 1 record

The JSON envelope contains a schema version, generation, records, and SHA-256 checksum. Each record
contains:

| Field | Purpose |
| --- | --- |
| `schema_version` | Record schema version, currently `1` |
| `execution_id` | Opaque local identifier |
| `origin` | New thread or selected existing thread |
| `known_thread_id`, `known_turn_id` | Exact validated Codex identities, if received and durably stored |
| `mutation_intent` | The last externally visible mutation class |
| `execution_phase` | Last durable controller phase |
| `confirmation_state`, `warning_acknowledged` | User confirmation and warning acknowledgement, stored separately from execution outcome |
| `last_authoritative_status` | Last validated status reported by Codex |
| `created_at`, `updated_at`, `revision` | Local journal ordering and compare-and-swap metadata |
| `recovery_required` | Whether the task still needs reconciliation or explicit review |

Serde rejects unknown fields. The record has no prompt, response, reasoning, tool output, command,
approval body, diff, credential, authentication header, transcript, full workspace path, remote
URL, or raw App Server message field. Private execution IDs, thread IDs, and turn IDs remain
sensitive operational metadata.

The store is capped at 32 records and 64 KiB. A new record can prune resolved, cancelled, and
pre-mutation records. It never prunes an unresolved record; when unresolved work fills the bound,
new execution is refused with a specific error. P031/P032 token and quota tables are not modified.

## Write-ahead order and crash meanings

For every ambiguous mutation, the controller writes an intent phase and then a conservative
“may-have-been-sent” phase before calling the transport. Both writes complete their durability
barriers before the corresponding RPC begins. The durable may-have-been-sent phase means only that
the call was allowed to begin; it does not prove a provider request occurred.

| Crash point | Durable evidence after restart | Recovery behavior |
| --- | --- | --- |
| Before the execution record is created | No record; no mutation path has begun | No task was submitted by this controller |
| After intent, before the transport call | Intent / may-have-been-sent, no identity beyond any earlier known identity | Treat as possibly sent; do not replay |
| During RPC write, after bytes leave, or while awaiting a response | May-have-been-sent; returned identity may be absent | Unknown outcome; never retry the RPC automatically |
| After a thread response but before its ID is durable | Thread-start may-have-been-sent; thread identity unknown | Unknown thread; no identity is inferred |
| After thread identity is durable, before turn intent | Known thread, unknown turn | Preserve the warning; no replacement turn is started |
| After turn response but before its ID is durable | Known thread, unknown turn; turn-start may-have-been-sent | Outcome unknown; do not guess from time or content |
| After turn identity and running state are durable | Exact thread and turn IDs | User may request read-only reconciliation |
| During approval response | Approval-response may-have-been-sent; request contents are not stored | Do not reconstruct or resend an approval; reconcile the known turn only |
| During interrupt | Interrupt intent, may-have-been-sent, or acknowledged | Never automatically send another interrupt; inspect current turn state |
| After a terminal event but before terminal journal write | Prior known running/ambiguous record remains | Read-only reconciliation may recover the terminal status |

The distinction between an intent and a may-have-been-sent marker narrows diagnostics, but both are
treated conservatively if found after restart. Missing acknowledgement does not prove rejection.
This design provides durable evidence and duplicate-submission prevention; it does not claim
exactly-once execution.

## Startup and explicit recovery

Startup reads and validates the journal without performing an App Server mutation, before drawing
the first interactive screen. The TUI opens a recovery view for unresolved entries, so a slow App
Server connection cannot leave a keyboard window before the warning appears. The ordinary
dashboard remains available after the user returns from recovery.
Thread and turn IDs are hidden in the list; the user can open the local detail view to inspect them.
`r` requests reconciliation, `a` acknowledges the warning, `i` explicitly requests an interrupt
only after a fresh read verifies that the exact known turn is still running, and `Esc` returns to
the dashboard. Recovery shortcuts are isolated from Composer submission keys.

Acknowledgement is not an execution result. It does not clear `recovery_required`, invent a terminal
status, or delete the record. It records that the user reviewed the warning and allows a later new
submission to proceed deliberately. A journal or schema error fails execution closed and remains
visible while read-only dashboard features continue.

Reconciliation uses the P036 read-only client, requires the audited App Server runtime, reads the
exact known thread, pages a bounded thread-turn list, and matches the exact durable turn ID. A
confirmed result is one of recovered completed, failed, interrupted, or still running. A missing
thread/turn identity, a wrong identity, an unavailable runtime, or missing server evidence remains
unknown or unavailable. When the thread is known but the turn ID is not, this build does not guess
which turn belongs to the execution. When the thread is unknown, it does not search by workspace,
prompt, or timestamp.

Approvals are never replayed or granted after restart. A fresh approval request is accepted only
through the active P036 broker after it verifies thread, turn, request, item, and scope. A turn
interrupt is never resent at startup. If explicit reconciliation reports that the exact turn is
still running, the user can choose `i`; the new interrupt has its own write-ahead record and its
acknowledgement remains distinct from authoritative interruption.

## Tests and terminal behavior

The Unix PTY integration tests launch the compiled `codex-meter` binary with isolated per-test
`HOME` and XDG data directories and a synthetic `codex app-server` executable injected only into
that test process's `PATH`. They exercise Unicode prompt submission, completion, allow-once and deny approvals,
interrupt acknowledgement followed by terminal interruption, connection loss and read-only
reconciliation, SIGKILL during active execution, SIGKILL after possible `thread/start` and
`turn/start` transmissions, restart detection, request counts, and no replay. They run with
`NO_COLOR=1` and `LC_ALL=C`, resize the PTY, and compare terminal mode and control sequences after
normal exit.

SIGKILL bypasses application cleanup. The crash tests explicitly restore their own pseudo-terminal
before starting the second process; they do not claim the killed process restored raw mode or the
cursor. On a real terminal after a forceful kill, use the shell's `reset` command or `stty sane` if
the terminal remains in raw mode. Normal exit, Ctrl+C, dirty Composer confirmation, recovery-view
exit, and execution terminal paths use the application cleanup guard.

The Windows ConPTY integration tests launch the compiled binary inside a native pseudo console. A
fixture `codex.exe` starts a synthetic local App Server, allowing approval denial, interruption,
parent termination, restart reconciliation, and no-replay checks without sending a real prompt to
Codex. Windows App Server descendants are placed in a Job Object configured to kill the job when
Codex Meter closes its handle. This covers process termination; it does not prove behavior after a
Windows kernel crash or sudden power loss.

Unix PTY and Windows ConPTY tests exercise synthetic App Server behavior, not a live Codex task.
Native P037 macOS x86_64 and ARM64 CI passed the Unix journal and PTY suites; P038 regression
results are tracked in the [platform matrix](V02_EXECUTION_RECOVERY_PLATFORM_MATRIX.md). The live
Arch reference-device gate remains separate and unverified.

## Provenance and limits

The journal does not read or replace Codex rollout/session files, P031/P032 SQLite evidence, or P035
saved drafts. No task token attribution, quota delta, model cost, or account-wide usage claim is
derived from its records. `turn/completed` without matching token evidence is not a token-ledger
entry; no journal record is not proof that Codex had no activity.

Windows flushes journal data and requests write-through replacement, but this is not a guarantee
against every storage-controller cache, filesystem, or hardware power-loss scenario. Process-crash
tests do not establish power-loss durability. See the
[cross-platform execution matrix](V02_EXECUTION_RECOVERY_PLATFORM_MATRIX.md) for current runtime
evidence and open release gates. The Arch reference gate remains outstanding; this journal must not
be used to infer task-token attribution or forecast quota.
