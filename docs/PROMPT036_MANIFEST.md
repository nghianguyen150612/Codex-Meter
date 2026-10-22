# PROMPT036 implementation manifest

## Status and baseline

- Status at P036 merge: **PARTIAL**. The execution controller and fake-server tests were
  implemented, while the reference Arch device, durable crash recovery, and PTY end-to-end gates
  remained outstanding. P037 adds the journal and PTY coverage; the Arch device gate remains
  pending. Historical P036 validation entries below are preserved as P036 evidence.
- Verified starting `origin/main`: `6f60a157f3eacf94f7abe547a317f34a27916bb3`, the P035 merge
  commit from PR #30.
- Feature branch: `feat/p036-native-task-execution`.
- PR title: `Implement native Codex task execution and approval lifecycle`.
- PR: [#31](https://github.com/nghianguyen150612/Codex-Meter/pull/31), merged 2026-10-22 at
  `18225c724813d85eaa76324e3f3674d116eee9a7`.
- No changes are made directly to `main`. The published v0.1.0 release/tag and merge-commit
  history remain intact. P036 does not publish v0.2.0.
- Installed Codex CLI audited: `codex-cli 0.163.0`.
- Corresponding upstream revision: `83086188ae0614bd882818ff1aa23f3194962012`, tag
  `rust-v0.163.0`.
- The stable generated App Server JSON Schema was produced using
  `codex app-server generate-json-schema --out <directory>` without `--experimental` and without
  enabling `experimentalApi`. The CLI labels the app-server command experimental; the operations
  used by P036 are present in that stable schema output. This audit does not guarantee forward
  compatibility.
- Reference Arch Linux device was unavailable. No real session was resumed and no paid task was
  submitted.

## Implementation

The App Server transport now distinguishes `ReadOnlyManagement` and `InteractiveExecution`. The
read-only capability retains P033/P034 request behavior, discards notification payloads as before,
and rejects server-initiated requests. The interactive capability has an explicit method and event
allowlist, retains bounded typed payloads, accepts only command/file/permission approval requests,
and exposes explicit response and rejection operations. Queues and request counts are bounded;
critical lifecycle/approval loss, malformed critical events, and oversized critical events require
recovery. Noncritical drops are counted and shown to the user.

The execution layer consumes P035 `PreparedTask` and runs fresh validation before submission. It
rechecks workspace existence, target identity/idle state, live model/effort support, and safe
configuration. Inherited model and reasoning values stay unresolved until Codex reports them.
Existing-thread execution has a separate inspect/resume confirmation followed by a second submit
confirmation. New-thread execution creates a thread only after confirmation, validates the server
response, preserves a known thread ID if turn start later fails, and does not fork or replay.

The typed execution state machine distinguishes ready, confirmation, submission, accepted,
running, approval, interrupt-requested, reconciliation, completed, failed, interrupted, and unknown
states. A submission response is not completion evidence. Terminal success/failure/interruption
comes from authoritative turn state. Timeouts, process loss, malformed responses after a mutation,
and critical event loss become unknown rather than failed. Reconciliation reconnects through
App Server, resumes the exact known thread without loading turn items, pages the bounded turn list,
and matches the exact turn ID. No uncertain request is resent.

The approval broker validates exact request ID, thread, turn, item, requested scope, and available
decisions. It never auto-approves or grants session-wide permissions. Command and file decisions
are limited to one-time accept, decline, and cancel. Permission responses include only validated
requested permissions at turn scope. File approval is disabled when a requested session-wide root
cannot be supported or a validated diff is absent. Network approval is labeled separately.

New-thread execution requires verified supported sandbox and approval settings, no network access,
and no extra writable roots. Existing-thread settings are preserved and checked after resume. The
controller refuses unknown or broad configurations, including unrestricted filesystem access and
approval policy `never`, without modifying global configuration or expanding permissions.

The execution overlay shows authoritative state, thread/turn identity, activity, bounded user-facing
agent output, tool output, proposed diff, approvals, interruption, unknown-state reconciliation,
and a final result summary. It does not display internal reasoning or persist prompts, transcript,
tool output, or RPC payloads. Existing P031/P032 quota samples may be displayed with timestamps and
freshness; task cost and per-turn token attribution remain unavailable. No telemetry sync is
started by execution.

P036 does **not** implement a durable crash-recovery journal or startup recovery detection. After
an abrupt process exit the user must inspect Codex's session before manually deciding whether to
submit again. The TUI cannot offer safe detachment; it explains that closing the terminal may not
stop the server-side turn and allows continued monitoring or an explicit interrupt request.

## Fake App Server evidence

The existing fixture now verifies new and existing thread operation order and exact request
parameters, active-turn refusal, command/file/permission approvals, approval denial on interrupt,
interrupt acknowledgement followed by terminal interruption, lost thread/turn-start responses,
model configuration changes, terminal start responses, missing final output, and reconciliation of
a known turn after connection loss. The reconciliation test confirms that the same configured
server command is reconnected, the exact thread/turn is inspected, the terminal state is recovered,
and `turn/start` is not replayed. Transport tests cover read-only server-request rejection,
interactive typed notification/server-request delivery, nonblocking approval response while an RPC
is pending, unknown-request rejection, bounded queue overflow, oversized critical payload recovery,
concurrent/out-of-order responses, process exit, timeout, broken pipe, malformed/oversized
messages, stderr draining, and child cleanup. These tests use synthetic prompts and do not contact
Codex or consume quota.

## Validation record

| Gate | Status |
| --- | --- |
| Starting `origin/main` | Verified at `6f60a157f3eacf94f7abe547a317f34a27916bb3` |
| Codex CLI / schema audit | `0.163.0`, stable schema generated with no experimental fields flag; upstream `83086188ae0614bd882818ff1aa23f3194962012` |
| Default locked Rust tests | Passed: 453 library + 4 analytics CLI + 19 sync CLI + 5 TUI CLI. |
| All-features locked Rust tests | Passed: 451 library + 2 analytics CLI + 19 sync CLI + 5 TUI CLI. |
| Rust Clippy | Passed with all targets/features and `-D warnings`. |
| Rust build | Passed: locked workspace build. |
| Python tests | Passed: 638 tests. |
| Ruff lint and format | Passed: `ruff check` and `ruff format --check`. |
| Python lock verification | Passed: `uv lock --project python --check`. |
| Contract validation | Passed: 6 schemas, 16 positive fixtures, 5 negative fixtures. |
| P029 installed-runtime smoke | Passed with the local Linux debug binary and synthetic SQLite evidence. |
| P030 version policy | Passed: product version remains `0.1.0`. |
| P030 release-candidate smoke | Passed locally for source `261b27d6ecedef89577d6d49d0d267ad54f7eb51`: Linux x86_64 candidate extracted, relocated, and exercised with the installed worker and synthetic evidence. |
| GitHub Actions CI | Passed for PR head `1c4584e8ab57e812b1b904852e8255e63fec0766`: 15 jobs in run `39714855760`, including Linux, macOS, and Windows Rust jobs and Python 3.11/3.12/3.13 installed-runtime matrix. |
| GitHub P030 release-candidate matrix | Passed for PR head `1c4584e8ab57e812b1b904852e8255e63fec0766`: all 6 jobs in run `39714855728`, including Linux x86_64, macOS x86_64/ARM64, Windows x86_64, and aggregate provenance verification. |
| P031/P032/P033/P034/P035 regressions | Passed in default and all-features workspace suites. |
| PTY execution-view validation | Pending; automated render and key-isolation tests pass. |
| Git diff review / `git diff --check` | Pending final pass. |
| GitHub Actions | Passed on Draft PR #31 head `1c4584e8ab57e812b1b904852e8255e63fec0766` (CI + release-candidate workflows) |
| Linux/macOS x86_64/macOS ARM64/Windows x86_64 | Rust CI and release-candidate builds passed on all four platform targets; installed-runtime Python matrix passed on Linux, macOS, and Windows |
| Arch live-device validation | **Pending; reference device unavailable** |
| PR / merge / release | PR #31 merged with merge commit `18225c724813d85eaa76324e3f3674d116eee9a7`; no v0.2.0 release authorized |

## P037 implementation follow-up

P037 implements the private identifier-only journal and Unix PTY restart coverage; see
`V02_EXECUTION_RECOVERY.md` and `PROMPT037_MANIFEST.md`. The Arch device compatibility gate remains
pending. P038 should complete Windows private storage and terminal validation and the reference Arch
read-only gate. Do not add quota forecasting, Savings Goals, plugin packaging, cloud execution, or
v0.2.0 release work under P036.
