# Codex Meter TUI dashboard, Composer, task execution, and recovery (P034–P037)

P034 adds an interactive Rust terminal dashboard over the existing P031 telemetry ledger, P032
reporting queries, and P033 Codex App Server services. P035 adds task preparation through the
Composer. P036 adds an explicitly confirmed execution overlay around Codex App Server. P037 adds
startup detection and explicit recovery for unfinished execution records. Codex stays
the agent runtime. These are unreleased development features and are not part of the published
v0.1.0 assets.

## Run

From a development checkout:

```console
cargo run --manifest-path rust/Cargo.toml -- tui
cargo run --manifest-path rust/Cargo.toml -- compose
cargo run --manifest-path rust/Cargo.toml -- tui --help
cargo run --manifest-path rust/Cargo.toml -- tui --database "local evidence.sqlite3"
```

The command requires interactive stdin and stdout. Redirected invocation exits with a `tui_requires_tty`
diagnostic before writing terminal control sequences. With no database option, the same per-user
SQLite database used by `codex-meter sync` and the reporting commands is read. Read-only views never
create a missing database or apply migrations. The optional path is used consistently for read
operations and for an explicitly confirmed sync.

At startup, the overview database read and one App Server session query run in bounded background
workers. A separate journal snapshot checks for unresolved execution records; it does not start a
turn, resume a thread, approve a request, or send an interrupt. A missing Codex executable leaves the telemetry overview available and reports session,
project, and model features as unavailable. No telemetry sync runs automatically. There is no
periodic refresh.

## Views and controls

The six screens are Overview, Sessions, Projects, Models, History, and Composer. Wide terminals use a
sidebar. Medium terminals use a tab strip, and narrow or short terminals use a compact active-view
label and shorter help line. Lists render at most 50 rows per screen page. The history query itself
uses the existing deterministic SQLite keyset cursor and reads only 50 records at a time.

| Key | Action |
| --- | --- |
| `Tab` / `Shift+Tab` | Next or previous screen |
| `1`–`6` | Open a screen directly when outside text editing |
| `Up` / `Down`, `j` / `k` | Move the selection |
| `n` / `p`, `PageDown` / `PageUp` | Next or previous list/history page |
| `Enter` | Open safe metadata or inspect selected evidence |
| `Esc` | Close a detail or confirmation dialog |
| `/` | Search the current screen's displayed metadata |
| `m` (History) | Set an exact model filter for token history |
| `w` (History) | Cycle Today, Week, and All history |
| `t` (History) | Switch between token and quota history |
| `f` (Sessions) | Cycle non-archived, archived, and all sessions |
| `a` (Sessions) | Toggle the active runtime-state filter |
| `L` (Sessions) | Explicitly load the selected thread |
| `F` (Sessions) | Open the fork confirmation dialog |
| `c` (Sessions) | Open Composer with the selected session as target; no resume or transcript read |
| `r` | Refresh the current view |
| `s` | Open the local telemetry sync confirmation |
| `F6` | Open unresolved execution recovery from the dashboard or Composer |
| `?` | Show help |
| `q` or `Ctrl+C` | Exit and restore the terminal |

While search is active, printable keys edit the search field and do not trigger global shortcuts.
Text search filters the currently loaded, privacy-safe metadata. History model filtering uses the
exact model value accepted by P032 reporting. Session, project, and model screens retain selection
when results update where the selected page still exists.

On the Composer screen, Tab moves between form fields; Tab from Actions advances to the next
screen, while Shift+Tab from Target moves to the previous screen. Focus the Actions row to use its
Save Draft, Load Draft, Templates, and Review Preflight choices without Ctrl-key support. Composer
shortcuts and editor behavior are documented in the [Composer guide](V02_PROMPT_COMPOSER.md).

### Overview

The overview reads P032's typed token-statistics and quota-timeline APIs. It displays directly
observed raw totals and token components, canonical per-response count, missing raw-total count,
five-hour and weekly quota samples, observation times, observed reset times, and source coverage
diagnostics. Active, idle, not-loaded, and other local session counts come from P033 metadata.

Quota bars use ASCII `#`/`-` glyphs for broad terminal compatibility. The numeric used percentage
is shown at its validated value; only the graphical fill is bounded to the bar width. Remaining
percentage is derived as `100 - used` and is labeled separately. Missing samples display
`Unavailable`, invalid measurements display `Invalid measurement`, and observations older than the
24-hour presentation policy display `Stale`. Reset time is shown only when observed. No unofficial
token capacity is presented as a provider quota limit.

### Sessions

The session list, filters, and details use P033's existing local App Server services. Rows are
sorted by reported update time, newest first, and show thread ID, runtime status, source, model, and
update time. Details distinguish thread ID from session-tree ID; they include archive state, source,
model/reasoning evidence, project association, parent/fork relationship, history availability, and
usage-link availability. No full transcript is loaded automatically.

P033 currently reports `usage_link: unavailable`, so details always state:

> Session token attribution: Unavailable

The TUI does not infer a token link from title, path, timestamps, or similar IDs. `L` calls the
existing `thread/resume` service: it loads the thread only, without starting an interactive Codex
terminal, submitting a prompt, or creating a turn. `F` requires confirmation that the saved
conversation history will be cloned into a new thread; cancellation creates nothing. Neither action
changes model, sandbox, or approval settings. No `turn/start` operation is implemented.

### Projects

Project rows reuse P033's grouping and show response-local alias, session count, association kind,
and known repository/worktree counts. Aliases such as `project-001` are temporary labels for the
current service response, not persistent project IDs. Missing or incomplete associations remain
unavailable. Paths, working directories, and Git remotes are never rendered or persisted in the
usage database.

### Models

The models view uses the current live P033 App Server catalog. It shows reported model IDs and names,
default/hidden state, and a details panel for supported reasoning efforts, default effort, and input
modalities when returned. The screen states that catalog metadata does not guarantee account
entitlement or execution access. The TUI does not switch models or send model requests.

### History

History defaults to All history, ordered newest first. The token view uses P032's canonical
per-response records only. Incomplete rows remain visibly incomplete; snapshots are not added to
consumption totals. `Enter` performs a read-only evidence inspection. The quota subview displays
observed account-meter snapshots separately and never sums them as task usage.

### Composer

Composer is the sixth screen and `codex-meter compose` opens the same TUI there directly. Its
multiline prompt editor has a 64 KiB UTF-8 byte limit, accepts bracketed paste, rejects oversized
input without truncation, and keeps edits in memory by default. The live model picker uses P033's
`model/list` catalog and displays reported defaults, hidden state, reasoning capabilities, and input
modalities. Reasoning choices are tied to the selected model. The target picker uses P033 session
metadata; it never resumes a thread or reads its transcript. A new target requires an explicit
workspace choice and does not create a thread.

While prompt, title, or model-filter text has focus, `q` is ordinary input. Press `Ctrl+C` to
request exit from those fields; unsaved task changes require confirmation.

`F5` refreshes session, model, and quota metadata on Composer. A refreshed catalog or target
invalidates any open preflight so the user can review the current evidence again.

Review Preflight shows the exact prepared configuration and prompt, latest local five-hour/weekly
quota observations with timestamps, resets and freshness, local source coverage, and readiness
limitations. Expected task quota cost remains unavailable without calibrated evidence. Preflight
uses read-only reporting and metadata calls; it does not call `thread/start` or `turn/start`. Press
`e` from the preflight to request a fresh execution preflight. See the next section for confirmation
and task execution behavior.

Saved drafts are user-owned documents, separate from telemetry SQLite. Prompt text stays in memory
until the user explicitly saves it; there is no autosave. On Unix-like systems, versioned plaintext
draft files are stored in the per-user data directory with private permissions, atomic replacement,
bounded storage, and no-follow file operations. Permissions are not encryption. Saving is disabled
where private storage behavior is not implemented. See the
[Composer guide](V02_PROMPT_COMPOSER.md) for exact storage, shortcuts, and P036 handoff details.

### Task execution

The `e` action performs a fresh preflight with an interactive App Server capability. It does not
start a task. An explicit confirmation is required before a new thread or turn can be created. An
existing thread requires an inspect/resume confirmation and then a separate turn confirmation.
Only idle threads can accept a new turn. The execution overlay shows event-based lifecycle status,
thread/turn IDs, user-facing streamed output, bounded activity, proposed file changes, explicit
approvals, interruption, and unknown-outcome reconciliation.

App Server request acceptance does not mean completion. The `i` key requests interruption and
waits for authoritative terminal state. Timeouts and lost lifecycle events become Outcome unknown;
`r` reconciles by exact known turn ID and never resubmits it. Startup validates recovery records
before the first interactive draw; unresolved records open a recovery view. There, `r` explicitly
reconciles, `a` acknowledges the warning without changing task
status, `i` sends a newly journaled interrupt only after fresh read-only validation reports the
exact turn is running, and `Esc` returns to the dashboard. `F6` reopens this view. Safe detachment
is unavailable, so the UI explains that closing the terminal may leave the server-side task running.

The interactive view displays only user-facing agent response text, relevant tool output, and
proposed changes; internal reasoning is not displayed. Those values and approval details are
transient and bounded in memory. They are not written to telemetry SQLite, saved drafts, the
recovery journal, or automatic exports. The journal persists private execution/thread/turn IDs,
mutation phases, acknowledgement, authoritative status, and timestamps only. An abrupt exit leaves
an evidence-backed recovery record when a mutation may have been sent; it never causes automatic
resubmission. See the [task execution guide](V02_TASK_EXECUTION.md) and [recovery guide](V02_EXECUTION_RECOVERY.md) for security and failure semantics.

## Data and provenance rules

The TUI keeps these domains separate:

```text
raw tokens != weighted/effective usage != plan quota
Local Codex telemetry != complete account usage
Session runtime state != task completion state
Quota snapshots != per-task quota consumption
App Server model catalog != guaranteed model entitlement
```

Displayed telemetry comes from the Rust-owned privacy-filtered P031/P032 SQLite store. Session,
project, and model metadata comes from P033's local Codex App Server interface. Read-only database
workers use `SqliteReadStore`, which opens the file read-only and enables SQLite query-only mode.
History pagination uses deterministic keyset cursors; snapshots are inspected independently from
canonical consumption events.

Quota freshness and source-version labels describe the observed local sample. No sample is not zero
usage. Partial metrics display their known sum and missing-event count, or `Unavailable` when no
numeric sum can be supported. Local collection is never described as complete account-wide
coverage. Codex Cloud and ChatGPT Work usage are outside verified scope. A local/account quota
discrepancy is not automatically evidence of provider overcharging.

## Refresh and sync

`r` refreshes the selected screen. App Server RPC, SQLite reports, and sync run off the rendering
thread through bounded worker queues. App Server calls use P033's finite RPC deadlines. Older
results are ignored after a newer request supersedes them; a failed source is shown as unavailable
without freezing the interface. The app holds one App Server connection per TUI session instead of
starting a process for each redraw. On exit, terminal cleanup runs before workers join; queued work
is abandoned, an active RPC completes or reaches its deadline, and the App Server child is closed
before the CLI returns. An explicit sync already in progress is allowed to finish its ingestion
transaction before the worker exits.

`s` first explains that the operation runs P031's one-shot local ingestion service and writes the
Codex Meter SQLite evidence ledger. The user must press `y` or `Enter` in the confirmation dialog.
The result reports response events added and coverage-warning count; the dashboard then reloads its
views. No network upload or cloud synchronization is performed.

## Privacy and terminal recovery

The TUI consumes sanitized P031/P032/P033 interfaces and a narrowly allowlisted P036 execution
event stream. It does not parse raw rollout payloads or display credentials, raw App Server JSON,
internal reasoning, working-directory paths outside the explicit task review, or Git remotes.
Prompt and execution output remain transient in-memory data except for drafts the user explicitly
saves. The recovery journal is separate from both drafts and telemetry and stores no transcript or
prompt content.

`NO_COLOR` disables semantic colors. Selection uses bold styling and text markers in lists, so the
active row remains interpretable without color. A Linux PTY test submits Unicode input under
`LC_ALL=C`. Session titles are not rendered; path-like metadata identifiers are
redacted, and unsafe thread identifiers cannot be resumed or forked. Terminal lifecycle uses
scoped cleanup for raw mode, alternate screen, and cursor visibility. Normal exit, `Ctrl+C`,
recoverable terminal errors, and Rust unwinding restore those settings. Mouse capture is not
enabled.

## Compatibility and validation limits

The telemetry parser remains verified for the audited Codex CLI `0.157.1` format. P036 generated
the stable App Server schema using Codex CLI `0.163.0` without the experimental fields flag; the
CLI still labels the App Server command experimental, and this audit is not a forward-compatibility
promise. A live empty catalog or missing Codex install is shown as unavailable/empty without
crashing. App Server requests and interactive events are bounded; the TUI pages ordinary lists at
50 rows.

No dedicated large-ledger or live rendering benchmark was run. Unit rendering tests cover wide,
medium, narrow, short, and low-height test backends; the history request remains bounded to 50
records. This is measured test coverage, not a claim about performance on every terminal or ledger.

The reference Arch Linux desktop was not attached to this implementation environment. Live startup,
real quota observations, model catalog, sandbox configuration, task confirmation, approval view,
resize, and shell restoration on that device remain an explicit gate; controlled fixtures do not
establish live compatibility. See the [P037 manifest](PROMPT037_MANIFEST.md) and
[compatibility matrix](V02_CODEX_COMPATIBILITY_MATRIX.md) for validation outcomes and platform
status.

## P038 handoff

The Unix P037 journal fails closed on Windows until private ACL-backed storage and replacement
semantics are implemented. The Arch live-device gate also remains pending. P038 should complete
those platform checks and any further audited runtime compatibility work. Quota forecasting,
Savings Goals, Codex Cloud adapters, plugin packaging, and cloud workspaces remain outside P037.
