# PROMPT035 implementation manifest

## Status and baseline

- Status: **PARTIAL** pending full validation, GitHub CI, and the reference Arch desktop gate.
- Verified starting `origin/main`: `00a98ba7bc5460b21ca7e3085d55eb9a31f0f2e0`, the P034 merge
  commit from PR #29.
- Feature branch: `feat/p035-prompt-composer-model-picker`.
- Suggested PR title: `Add interactive prompt composer, model selection, and task preflight`.
- PR URL: not created yet.
- Published v0.1.0 release/tag and existing commit history are preserved. P035 does not publish
  v0.2.0 or change versioned telemetry/analytics contracts.
- Codex CLI observed in the implementation environment: `0.163.0`. P033 previously verified
  basic App Server operations against `0.159.2`; neither observation is a permanent protocol
  guarantee.
- Official App Server documentation and the locally generated stable protocol schemas were
  consulted before implementation. The Composer reuses P033 RPC methods and adds no protocol fields.
- The reference Arch desktop is not available from this environment. Its Codex runtime, live
  model visibility, interactive behavior, and device-specific quota evidence remain unverified.

## Implementation

P035 extends the Ratatui app with a sixth Composer screen and `codex-meter compose`, while keeping
`codex-meter tui` and no-argument help behavior. The screen uses a real 64 KiB UTF-8 multiline
editor, bracketed paste events, cursor movement and scroll, responsive layouts, focus handling,
unsaved-exit confirmation, templates, and explicit save/load/delete dialogs.

Session targets reuse P033 metadata. Selecting a session does not resume it, read its transcript,
or change it. New-thread preparation requires an explicit current-working-directory choice and
does not create a thread. The raw working path stays in memory and is not displayed or written to
drafts. Project associations in `PreparedTask` are explicitly tagged as response-local aliases,
not persistent project IDs. Existing session availability must be revalidated by P036.

The model picker uses P033's live `list_models()` result, including server-reported hidden entries.
It exposes model ID/name, default/hidden status, reasoning efforts/default, and input modalities.
Hidden entries are not preselected. Reasoning can inherit Codex configuration, use a selected
model's reported default, or explicitly select a supported effort. A catalog change that removes a
model/effort invalidates preflight and requires user review.

Preflight prepares a typed `PreparedTask`; it does not send a model request or create/resume a
thread. It displays local quota evidence with timestamps, reset times, freshness, and coverage
limitations. Task cost is always reported unavailable until suitable calibrated evidence exists.
SQLite access remains read-only and uses P032 reporting; there is no second usage ledger or quota
prediction implementation.

Prompt text is in memory by default. Explicitly saved drafts use a separate Unix per-user directory,
versioned JSON, private permissions, no-follow file access, atomic replacement, bounded storage,
and conflict detection. Saved content is plaintext unless the user's filesystem independently
encrypts it. Non-Unix draft persistence fails closed with an actionable message.

P035 Composer and preflight code do not dispatch `thread/start`, `turn/start`, `thread/resume`, or
`thread/read`; existing P034 session actions remain unchanged. Source and test review confirms no
preflight path reaches a thread or turn mutation. The fake App Server fixture exits with an error
if a test sends `thread/start` or `turn/start`, so accidental calls fail the test.

## Validation record

| Gate | Status |
| --- | --- |
| Rust formatting | Passed: workspace `cargo fmt --check` |
| Clippy | Passed: workspace, all targets/features, warnings denied |
| Default locked Rust tests | Passed: 410 library + 4 analytics CLI + 19 sync CLI + 5 TUI CLI (438 total) |
| All-features locked Rust tests | Passed: 408 library + 2 analytics CLI + 19 sync CLI + 5 TUI CLI (434 total) |
| Locked Rust build | Passed: workspace build |
| Python tests | Passed: 638 |
| Ruff lint and format | Passed |
| Python lock verification | Passed: frozen sync and `uv lock --check`; lockfile unchanged |
| Schema/fixture validation | Passed: 6 schemas, 16 positive fixtures, 5 negative fixtures |
| P029 installed-runtime smoke | Passed: reproducible wheel, isolated install, relocation, deterministic output, concurrency, read-only SQLite |
| P030 release version policy | Passed: `0.1.0` policy unchanged |
| P030 local candidate packaging smoke | Pending clean committed checkout; packaging intentionally refuses a dirty source tree |
| P031/P032/P033/P034 regressions | Passed in default and all-features Rust workspace suites |
| Controlled PTY Composer smoke | Passed with a failing fake Codex executable and temporary paths: startup, bracketed paste, dirty-exit confirmation, terminal cleanup, and no SQLite creation |
| `git diff --check` | Passed before final commit review |
| Linux/macOS/Windows GitHub Actions | Pending PR |
| Arch live-device validation | **Pending; reference device unavailable here** |

No paid model request or private session resume was used during validation. The PTY smoke used a
fake failing Codex executable and did not query real sessions. Sanitized runtime reports omit
prompt text, thread IDs, filesystem paths, and credentials.

## Future P036 execution handoff

P036 should consume `PreparedTask` after explicit user confirmation. It must revalidate an existing
thread's availability and new-workspace/project semantics, resolve inherited Codex settings without
claiming unknown values, review effective sandbox and approval-policy behavior, then own the
execution RPC lifecycle, approval handling, turn events, cancellation, and final state. P036 must
not treat a successful draft save or P035 preflight as submission or execution.
