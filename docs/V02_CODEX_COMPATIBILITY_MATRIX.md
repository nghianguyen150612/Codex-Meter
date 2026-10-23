# Codex CLI and App Server compatibility (P037)

Execution uses a narrow runtime allowlist. A generated or checked-in schema match is useful
evidence, but it does not establish that a particular installed App Server handles execution,
approval, interruption, and read-only reconciliation end to end.

## Matrix

| Runtime | Schema evidence | Fake protocol / PTY coverage | Live read-only reference Arch | Execution enabled |
| --- | --- | --- | --- | --- |
| `codex_cli_rs/0.163.0` (`rust-v0.163.0`, commit `83086188ae0614bd882818ff1aa23f3194962012`) | P036 generated the stable schema with the installed 0.163.0 CLI; initialize, thread start/resume/read/list, turn start/interrupt/completion, approvals, and security settings were audited. | P036 fake App Server tests retained; P037 Unix PTY fixture identifies itself as 0.163.0 and exercises submission, approvals, interruption, connection loss, restart, and reconciliation. This is synthetic protocol evidence, not a live Codex run. | **Pending**. The reference Arch device is unavailable. | **Yes**, only when initialization identifies this exact audited runtime and the other fresh preflight/security checks pass. |
| `codex_cli_rs/0.162.1` (`rust-v0.162.1`, commit `092d3acd6bec3e3a14bdc7e7a2810ab628ab759d`) | Targeted checked-in upstream protocol schemas compared with 0.163.0. Initialize, thread start/response, thread resume response, turn start response, turn started/completed, interrupt, approval request/response, turn-list, and server-request-resolved schemas match. `ClientRequest`, `ServerNotification`, `ThreadResumeParams`, `TurnStartParams`, and `ThreadReadResponse` differ; 0.163.0 adds conversation-item fields, an optional `retain` field, thread unread-state data, and a new notification. | **Not run against a 0.162.1 binary.** The existing fake server is a 0.163.0 fixture and does not establish 0.162.1 runtime behavior. | **Not run**. No 0.162.1 runtime is installed on the reference Arch device. | **No**. The stable schema subset is not enough to verify response semantics, security-policy behavior, interrupt handling, or reconciliation. |
| Other stable tags available upstream, including `0.162.0`, `0.161.0`, and `0.160.x` | Not audited for P037. | Not run. | Not run. | **No**. |
| `0.164.0-alpha.*` and other development builds | Not audited; prerelease tags are not considered stable compatibility evidence. | Not run. | Not run. | **No**. |

The upstream tags endpoint showed `rust-v0.162.1` as the closest prior stable tag and `rust-v0.164.0-alpha.*` as current development tags on 2026-10-22. The local `codex --version` reports `codex-cli 0.163.0`; `codex app-server --help` was checked in the implementation environment. These checks do not satisfy the Arch live-device gate.

The P036 runtime gate remains the exact 0.163.0 allowlist. It is not a numerical minimum and is not broadened based on static schema comparison.

This matrix governs Codex protocol versions. OS execution and persistence qualification is recorded
separately in the [P038 execution recovery platform matrix](V02_EXECUTION_RECOVERY_PLATFORM_MATRIX.md);
the Codex 0.163.0 allowlist does not by itself certify Windows, macOS, Linux filesystems, or a live
reference device.

## Protocol review scope

The stable P036 baseline was audited against upstream Codex tag `rust-v0.163.0`. P037 reviewed the
closest prior stable source schema at `rust-v0.162.1` for initialization, thread creation/resume,
turn submission and completion, interruption, command/file/permission approvals, thread reads, and
bounded turn listing. The targeted comparison found that the main turn acknowledgement, completion,
interrupt, approval request/response, and turn-list shapes are stable across these two tags. It also
found protocol evolution in client conversation items, resume inputs, the nested turn input union,
thread-read metadata, and server notifications.

No runtime compatibility claim is made for 0.162.1. The differences need an installed binary, a
version-specific fake-server run, read-only live validation, and review of sandbox/approval
semantics before the allowlist can include it. Older stable versions are not assumed compatible;
newer stable and prerelease versions are not assumed compatible either.

## Reference Arch gate

When the device is available, record the exact output of `codex --version` and `codex app-server
--help`, then run the P037 read-only TUI, Composer, session, model, project, telemetry, quota, resize,
and terminal-restoration checks from the milestone prompt. Do not send a paid prompt, resume/fork a
personal session, or modify Codex configuration during this gate. Controlled fake-server tests
remain the source for synthetic task execution, approval, interruption, crash, and restart paths.

See [PROMPT037_MANIFEST.md](PROMPT037_MANIFEST.md) for the consolidated device and platform gates,
and [V02_EXECUTION_RECOVERY.md](V02_EXECUTION_RECOVERY.md) for recovery guarantees.
