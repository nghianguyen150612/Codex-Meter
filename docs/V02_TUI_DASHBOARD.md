# Codex Meter TUI dashboard (P034)

P034 adds an interactive Rust terminal dashboard over the existing P031 telemetry ledger, P032
reporting queries, and P033 Codex App Server services. It is a local observation and session
management interface. It does not replace the Codex agent runtime and does not submit model turns.
The TUI is unreleased development functionality and is not part of the published v0.1.0 assets.

## Run

From a development checkout:

```console
cargo run --manifest-path rust/Cargo.toml -- tui
cargo run --manifest-path rust/Cargo.toml -- tui --help
cargo run --manifest-path rust/Cargo.toml -- tui --database "local evidence.sqlite3"
```

The command requires interactive stdin and stdout. Redirected invocation exits with a `tui_requires_tty`
diagnostic before writing terminal control sequences. With no database option, the same per-user
SQLite database used by `codex-meter sync` and the reporting commands is read. Read-only views never
create a missing database or apply migrations. The optional path is used consistently for read
operations and for an explicitly confirmed sync.

At startup, the overview database read and one App Server session query run in bounded background
workers. A missing Codex executable leaves the telemetry overview available and reports session,
project, and model features as unavailable. No telemetry sync runs automatically. There is no
periodic refresh.

## Views and controls

The five screens are Overview, Sessions, Projects, Models, and History. Wide terminals use a
sidebar. Medium terminals use a tab strip, and narrow or short terminals use a compact active-view
label and shorter help line. Lists render at most 50 rows per screen page. The history query itself
uses the existing deterministic SQLite keyset cursor and reads only 50 records at a time.

| Key | Action |
| --- | --- |
| `Tab` / `Shift+Tab` | Next or previous screen |
| `1`–`5` | Open a screen directly |
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
| `r` | Refresh the current view |
| `s` | Open the local telemetry sync confirmation |
| `?` | Show help |
| `q` or `Ctrl+C` | Exit and restore the terminal |

While search is active, printable keys edit the search field and do not trigger global shortcuts.
Text search filters the currently loaded, privacy-safe metadata. History model filtering uses the
exact model value accepted by P032 reporting. Session, project, and model screens retain selection
when results update where the selected page still exists.

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

The TUI consumes only sanitized P031/P032/P033 interfaces. It does not parse raw rollout payloads
or display prompts, responses, reasoning text, tool content, credentials, raw App Server JSON,
working-directory paths, or Git remotes. Error text is reduced to bounded local error categories.
Session metadata and UI state are not persisted.

`NO_COLOR` disables semantic colors. Selection uses bold styling and text markers in lists, so the
active row remains interpretable without color. Non-UTF-8 locales receive ASCII borders and
sanitized displayed labels. Session titles are not rendered; path-like metadata identifiers are
redacted, and unsafe thread identifiers cannot be resumed or forked. Terminal lifecycle uses
scoped cleanup for raw mode, alternate screen, and cursor visibility. Normal exit, `Ctrl+C`,
recoverable terminal errors, and Rust unwinding restore those settings. Mouse capture is not
enabled.

## Compatibility and validation limits

The telemetry parser remains verified for the audited Codex CLI `0.157.1` format. The App Server
interface is described upstream as experimental/unsupported for production; this TUI inherits
P033's protocol compatibility limits. A live empty catalog or missing Codex install is shown as
unavailable/empty without crashing. P033's service response limits bound session and model metadata;
the TUI pages rendering at 50 rows, while the service may hold a larger bounded result in its worker.

No dedicated large-ledger or live rendering benchmark was run. Unit rendering tests cover wide,
medium, narrow, short, and low-height test backends; the history request remains bounded to 50
records. This is measured test coverage, not a claim about performance on every terminal or ledger.

The reference Arch Linux desktop was not attached to this implementation environment. Live startup,
real quota observations, resize, session views, and shell restoration on that device remain an
explicit gate; controlled fixtures do not establish live quota accuracy or compatibility. See the
[P034 manifest](PROMPT034_MANIFEST.md) for validation outcomes and PR state.

## P035 handoff

P034 stops at observation and guarded session lifecycle actions. Prompt composition, model
switching, quota prediction, Codex Cloud adapters, persistent project management, PR/CI integration,
plugin packaging, and cloud workspaces remain future milestones requiring their own evidence and
scope.
