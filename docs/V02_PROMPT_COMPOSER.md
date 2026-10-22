# Prompt Composer and Model Picker (P035)

P035 adds a task preparation screen to the Ratatui application. It composes a prompt, selects a
new-thread or existing-thread target, uses live Codex App Server model capabilities, reviews local
quota evidence, and optionally saves an explicitly named draft. P035 preparation and saved drafts
remain separate from P036's execution flow. Opening or saving preflight never starts a task.

The feature is development functionality and is not included in the published v0.1.0 assets.

## Open the Composer

```console
cargo run --manifest-path rust/Cargo.toml -- compose
cargo run --manifest-path rust/Cargo.toml -- compose --database "local evidence.sqlite3"
```

`codex-meter compose` opens the same Ratatui application as `codex-meter tui`, with Composer
selected. The no-argument CLI continues to print help. Composer needs an interactive terminal.
Startup requests session metadata, the model catalog, and read-only local quota evidence in bounded
workers. Model/session discovery errors leave the prompt in memory and are shown in the UI.

The six screens are Overview, Sessions, Projects, Models, History, and Composer. Outside text
editing, `Tab` / `Shift+Tab` changes screens and `1`–`6` opens a screen directly. On Sessions, `c`
opens Composer with the selected thread ID as the existing target; it does not load or resume the
thread.

## Composer workflow

1. Select **New local Codex thread** or an existing local session in the Target field. Existing
   rows show their reported runtime state and project association where available. Project labels
   from P033 are response-local association metadata, not persistent project IDs. Archived and
   missing targets fail validation. A row in history is not proof that a new turn can be started;
   P036 must revalidate the target before execution.
2. For a new thread, choose **Use current working directory** explicitly (`Enter` or `w` in the
   Workspace field). The value is read from the process working directory. Its path stays in
   memory and is not displayed or saved in a draft; no thread or project association is created.
3. Choose a model from the live `model/list` result or select **Inherit Codex configuration**.
   Search matches model ID or display name. The details show reported default, hidden status,
   reasoning efforts, reported default effort, and input modalities when supplied. Hidden models
   are never preselected or recommended. An explicit hidden-model selection is visible in the
   review. Catalog presence is not an account entitlement or execution guarantee.
4. Choose reasoning behavior: inherit from Codex configuration, use the selected model's reported
   default, or select one of that model's supported efforts. If the model changes or a refreshed
   catalog removes an effort, the previous value is retained only as a warning and requires a new
   choice. P035 does not change global Codex settings or a running thread.
5. Write or apply a generic built-in template, then open **Review Preflight**. Preflight displays
   the exact prompt and the intended target/model/reasoning configuration. It does not call Codex
   to create, load, resume, or execute a task.

The prompt editor supports multiline insertion, Unicode-safe cursor movement, backspace/delete,
Home/End, and vertical scrolling. `Enter` inserts a newline. The prompt limit is 64 KiB measured in
UTF-8 bytes. An edit or paste that exceeds the limit is rejected in full; existing text is not
truncated. Bracketed paste events are delivered to the focused editor, title, or filter field and
are never interpreted as global shortcuts. Resize preserves the in-memory draft and cursor.

Built-in templates are generic starting text for Bug Fix, Feature Implementation, Code Review,
Testing, Documentation, and Refactoring. Applying a template only inserts editable text.

## Preflight evidence

Preflight is read-only. It shows:

- target kind, exact selected thread ID when present, reported runtime state, and project/workspace
  association status;
- selected model or inherited configuration, live catalog provenance, hidden/default status, and
  supported reasoning options;
- latest local five-hour and weekly account-meter used percentages, observation times, reset times
  when available, and freshness;
- local source coverage limitations, including unavailable or partial evidence;
- whether the prompt, target, workspace, App Server, model, and reasoning selections pass
  preparation validation; and
- `Expected task quota cost: Unavailable — insufficient calibrated evidence`.

Quota percentages are observed account-meter evidence, not raw-token capacity and not a task-cost
prediction. The interface does not estimate proposed-task cost from prompt length, tokens, or a
fixed percentage. Historical token usage and quota snapshots remain separate evidence. A stale
observation is labeled stale and is not current availability.

The typed `PreparedTask` retains prompt text and target/model/reasoning intent for P036. An inherited
model remains unresolved until Codex configuration resolves it. Its privacy-safe diagnostic summary
omits prompt text, thread IDs, project values, and workspace paths; the task itself is not serialized
or logged by default.

## Draft privacy and local storage

Prompt content stays in memory unless the user explicitly chooses **Save Draft** or `Ctrl+S` and
confirms a user-controlled title. There is no prompt autosave. Saved drafts can be listed, loaded,
replaced, or explicitly deleted. Loading into a dirty composer is refused so the current prompt is
retained. Closing with unsaved changes asks whether to quit, return, or save.

On Unix-like systems, drafts live in `$XDG_DATA_HOME/codex-meter/drafts`, or
`$HOME/.local/share/codex-meter/drafts` when `XDG_DATA_HOME` is unset. The directory is private
(mode 0700); draft and lock files use mode 0600. Storage uses versioned JSON, opaque filenames,
atomic replacement, a lock, digest-based conflict detection, and no-follow file operations. Limits
are 100 drafts, 10 MiB total, 128 KiB per file, and 64 KiB per prompt. Unsafe or malformed entries
are skipped. Drafts contain plaintext prompt content. File permissions reduce access by other local
users but do not encrypt content; rely on filesystem encryption if encryption is needed. There is
no automatic cloud sync.

Draft saving is disabled on platforms where private no-follow persistence is not implemented and
returns an actionable error. Draft documents are separate from the SQLite telemetry ledger and
never become token/quota evidence, diagnostics, or log entries.

## Keyboard reference

| Key | Action |
| --- | --- |
| `Tab` / `Shift+Tab` | Move between Composer fields; at field boundaries, continue to adjacent screen |
| `1`–`6` | Open a screen while focus is outside the prompt editor |
| `Enter` in prompt | Insert newline |
| Arrow keys, `Home` / `End` | Move the prompt cursor |
| `PageUp` / `PageDown` | Move the cursor by several prompt lines |
| `Ctrl+Enter` | Review preflight |
| `Ctrl+S` | Explicitly save a draft |
| `Ctrl+M` | Open model picker |
| `Ctrl+R` | Open reasoning picker |
| `Ctrl+D` | Open saved draft library |
| `F5` | Refresh session, live model, and quota metadata |
| `w` / `c` in Workspace | Select / clear current working directory |
| `s`, `l`, `t`, `r` in Actions | Save, load, templates, review preflight |
| `?` or `F1` outside prompt editing | Show help |
| `Esc` | Close a dialog or move focus between prompt and actions |
| `q` outside text entry / `Ctrl+C` anywhere | Quit; unsaved changes require confirmation |

The action bar is keyboard-focusable for terminals with limited Ctrl-key reporting. If a terminal
does not distinguish `Ctrl+Enter`, focus Actions and activate Review Preflight.

## App Server compatibility

Composer uses the existing P033 App Server discovery. It does not read credentials or create a
separate authentication/runtime. The installed runtime reported Codex CLI `0.163.0`; P033 had
previously validated basic operations against `0.159.2`. P036 separately audited stable generated
schemas against `0.163.0` before enabling an isolated execution capability. There is no fixed
supported-version range; protocol behavior can evolve and schemas do not guarantee future runtime
compatibility. See the [task execution guide](V02_TASK_EXECUTION.md) for methods, schemas,
confirmation, and the security boundary.

P035 task preparation remains read-only. The P036 execution flow consumes its `PreparedTask` only
after fresh validation and explicit user confirmation. The Arch reference desktop gate remains
pending; see the [P036 manifest](PROMPT036_MANIFEST.md) for validation status.

## P036 execution

After reviewing P035's preflight, press `e` to request a fresh execution preflight. This reads the
current App Server model catalog and checks the chosen workspace or existing thread and relevant
sandbox/approval settings. It does not create or resume a thread. The execution confirmation shows
the task and current security configuration. Existing targets use an additional inspect/resume step
and require a second confirmation before submission. Each model, workspace, target, or security
change invalidates the previous confirmation.

The exact prepared prompt is sent to `turn/start` only after explicit confirmation. Codex Meter
shows the accepted, running, terminal, and unknown states separately. Approval requests require an
explicit one-time allow/deny/cancel choice; the default is deny. The `i` key requests Codex
interruption, and `r` reconciles an unknown outcome by known turn ID. A timeout does not trigger a
retry. See the [P036 task execution guide](V02_TASK_EXECUTION.md) for protocol, sandbox, approval,
privacy, recovery, and keyboard details.

Execution output, prompt text, tool output, and diffs are transient. They are not saved to the P035
draft store or the token/quota evidence ledger. P036 does not implement a durable recovery journal;
after an abrupt exit, inspect the Codex session before deciding whether to submit again. This is a
known limitation and the Arch live-device validation gate remains pending. See the
[P036 manifest](PROMPT036_MANIFEST.md).
