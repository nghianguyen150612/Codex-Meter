# Native Codex task execution (P036)

P036 adds a guarded execution controller to the unreleased `codex-meter compose` TUI. Codex App
Server owns the agent runtime, authentication, sandbox enforcement, and provider connection. Codex
Meter validates the prepared task, obtains explicit confirmation, routes the supported RPC and
approval messages, and presents the server's lifecycle state. This does not change the published
v0.1.0 release.

## Start a task

1. Run `cargo run --manifest-path rust/Cargo.toml -- compose` and prepare a prompt, target, model,
   and reasoning choice using the P035 Composer.
2. Open the P035 read-only preflight with `Ctrl+Enter`; press `e` to request a fresh execution
   preflight. This opens an interactive App Server connection, refreshes the model catalog, checks
   the selected workspace or thread, and checks applicable security settings. It does not create,
   resume, or modify a thread.
3. Review the task confirmation. A new thread requires one confirmation. An existing thread first
   requires confirmation to resume/load it and inspect its current security settings, then a second
   confirmation before submitting the turn.
4. Codex Meter revalidates the task immediately before submission. A changed model or security
   setting invalidates the confirmation and asks for a fresh review. If checks pass, a new target
   uses `thread/start`; both target types then use `turn/start` with the exact prepared prompt.
5. Follow the execution overlay until Codex reports a terminal turn state. A successful `turn/start`
   response means that Codex accepted the submission; it does not mean that the task completed.

The composer, draft save, model picker, and opening preflight do not start a turn. The execution
overlay prevents duplicate submission while the RPC mutation is pending. Existing threads are
never forked automatically, and active turns are refused.

## App Server compatibility

The implementation was audited against Codex CLI `0.163.0`. Its stable JSON Schema bundle was
generated locally by `codex app-server generate-json-schema --out <directory>` without the
`--experimental` flag and without setting `experimentalApi`. The matching upstream Codex revision
is `83086188ae0614bd882818ff1aa23f3194962012` (tag `rust-v0.163.0`). The App Server command and
schema-generation subcommand label themselves experimental in CLI help; the task lifecycle methods
used here appear in the generated non-experimental schema. Execution preflight is enabled only when
the App Server initialization `userAgent` identifies this audited `0.163.0` runtime. Older, newer,
and development builds fail closed before the model catalog or confirmation dialog is enabled.
This is a versioned audit point, not a compatibility promise for future Codex releases.

Audited methods and messages include `initialize` / `initialized`, `thread/start`, `thread/read`,
`thread/resume`, `thread/status/changed`, `turn/start`, `turn/started`, `turn/completed`,
`turn/interrupt`, item start/completion and user-facing agent deltas, command output deltas,
`turn/plan/updated`, `turn/diff/updated`, the three `item/*/requestApproval` server requests, and
`serverRequest/resolved`. Reconciliation uses `thread/turns/list`. Unknown server requests are
rejected. Unsupported or malformed lifecycle messages require recovery instead of being treated as
success.

The read-only management client and interactive execution client have separate capability
allowlists. The read-only client cannot call thread/turn execution mutations and rejects
server-initiated requests. The interactive client adds only the verified methods above and the
approval categories implemented here; it does not automatically accept other App Server requests.

## Security and approvals

New-thread execution is enabled only when the effective configuration resolves to a supported
`read-only` or `workspace-write` sandbox, `untrusted` or `on-request` approval policy, and no
network access or extra writable roots. Unknown settings, `danger-full-access`, `never`, and
unsupported combinations fail closed. A new thread is explicitly created with the verified
configuration and user approval review. The returned configuration is checked before `turn/start`.

For an existing thread, the controller preserves its thread identity, checks that it is idle,
resumes it only after the first explicit confirmation, and verifies the security settings returned
by Codex before asking for final confirmation. It refuses to expand permissions or silently repair
configuration. A protocol or runtime version that does not expose the required safe facts cannot
start a turn.

Command execution, file changes, and permission requests use an explicit modal. The default action
is deny. The UI offers one-time allow only when the request details are validated; it never offers
session-wide approval. File-change approval is disabled if a requested write root would expand
session permissions or the proposed diff is unavailable. Permission approval returns only the
validated requested subset with turn scope. Network requests are labeled separately and show the
validated host and protocol when supplied. Unsupported or wrong-thread/turn requests are rejected.
Codex's later `serverRequest/resolved` notice removes a stale approval from the UI.

## Progress, output, and final state

The execution screen shows the thread and turn IDs, selected or observed model label, event-based
status, bounded activity timeline, user-facing streamed response, and optional tool output and
proposed diff details. It does not display internal reasoning. Stream output, tool output, diffs,
and activity remain in bounded memory only. They are not written to the telemetry database or
automatically exported. Codex may persist its own thread history.

Only `turn/completed` or a validated terminal turn returned from reconciliation establishes a final
state. An agent message that looks final is not enough. A failed turn may include a sanitized
failure category; the UI also handles completion without a final user-facing response. Token usage
attribution for a particular turn is unavailable because P033's session-to-token mapping is not
verified.

## Interrupt and recovery

The `i` action sends `turn/interrupt` with the exact known thread and turn IDs. The screen reports
that Codex acknowledged the request and waits for authoritative turn completion before labeling it
interrupted. Pending approvals are denied when an interrupt begins. The App Server child process is
not killed as the normal interrupt mechanism.

If a request times out after it may have been accepted, the task becomes **Outcome unknown**. The
controller never retries `turn/start` or creates a replacement thread automatically. Press `r` to
reconnect and inspect the known turn through Codex's thread/turn APIs. An unknown outcome remains
unknown if Codex cannot return an authoritative record. The UI says to inspect the Codex session
before submitting another task.

P036 does not persist an execution journal. Thread IDs, turn IDs, lifecycle state, prompt, output,
and approval details are transient in memory; prompts and transcripts are not written to the
telemetry SQLite database or saved-draft store. After an abrupt application crash, Codex Meter
cannot recover which submission was in progress. Inspect the Codex session in Codex before
manually deciding whether to submit another task. Startup discovery of an interrupted execution
and a private, identifier-only recovery journal remain limitations for a later milestone.

The exit dialog explains that closing the terminal does not necessarily stop a server-side turn.
Safe detachment is unavailable; the user can continue watching or request interruption and wait for
the final state. Terminal cleanup restores raw mode and screen state on errors and normal exit.

## Quota and telemetry

Confirmation and final summaries may show the last observed five-hour and weekly quota samples,
their observation/reset timestamps, and evidence freshness. They are not real-time per-task
consumption. Task cost is unavailable; token attribution is unavailable; no telemetry sync starts
automatically. Users can invoke the existing explicit P031 sync later. Token counts and quota-meter
movement remain distinct measurements.

## Controls

| Key | Action |
| --- | --- |
| `e` in Composer preflight | Fresh execution preflight |
| `Enter` / `y` in confirmation | Confirm once (or inspect an existing thread on the first confirmation) |
| `Esc` / `n` before submission | Cancel confirmation |
| `Up` / `Down`, `PageUp` / `PageDown` | Scroll output |
| `d` | Show or hide tool output and proposed changes |
| `a` | Open pending approvals |
| `a` in an approval | Allow once, when enabled |
| `Enter`, `n`, or `Esc` in an approval | Deny |
| `c` in command/file approval | Cancel the request |
| `i` | Request interruption |
| `r` in unknown state | Reconcile against Codex |
| `Enter` / `Esc` after a terminal state | Return to dashboard |
| `q` / `c` during a running task | Review exit/interruption choices |

The prompt editor consumes typing and paste as prompt content; its controls cannot submit a turn.

## Known limits and validation

App Server behavior can change independently of its generated schemas. The Arch reference desktop
was unavailable during P036, so its installed Codex version, live model catalog, security
configuration, and terminal behavior remain unverified. No paid task was submitted for validation.
Controlled fake-server tests exercise request ordering and terminal/approval scenarios, but they
do not replace live-device evidence. See [PROMPT036_MANIFEST.md](PROMPT036_MANIFEST.md) for the
validation record and PR status.

P037 should add a private versioned crash-recovery journal containing only execution identifiers,
state, and timestamps, plus startup reconciliation and tests. It should not persist prompt text,
transcripts, output, reasoning, tool arguments, credentials, or workspace paths. P037 should also
complete the Arch-device compatibility gate before any merge approval.
