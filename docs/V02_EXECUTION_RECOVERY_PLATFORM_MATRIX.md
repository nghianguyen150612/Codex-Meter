# Execution recovery platform matrix (P038)

This matrix separates implementation from native runtime evidence. A passing cross-compile or
synthetic protocol test alone does not qualify a platform for release. The Codex App Server runtime
allowlist remains `codex_cli_rs/0.163.0`; this document does not broaden that version allowlist.

## Capability matrix

| Capability | Linux | macOS | Windows |
| --- | --- | --- | --- |
| Interactive terminal | Unix PTY through the host terminal | Unix PTY through the host terminal | Crossterm console I/O hosted in a native ConPTY; the host owns the ConPTY |
| Private journal security | Current-user owned directory `0700`; files `0600`; no-follow component opens | Current-user owned directory `0700`; files `0600`; no-follow component opens | Protected DACL with only the current user SID granted access; owner, DACL, ancestor write access, file type, hard-link count, reparse status, and local fixed-volume path checked |
| Atomic persistence | Same-directory rename, file and directory `fsync` | Same-directory rename, file and directory `fsync` | `FlushFileBuffers`, `ReplaceFileW` for replacement, `MoveFileExW` with write-through for initial creation |
| Interprocess locking | `flock` plus process mutex | `flock` plus process mutex | `LockFileEx` byte-range lock plus process mutex; five-second acquisition bound; kernel releases the lock when handles close |
| Revision compare-and-swap | Shared journal CAS under lock | Shared journal CAS under lock | Shared journal CAS under lock |
| Approval and interruption | P037 synthetic Unix PTY scenarios passed | P037 native CI scenarios passed | Native ConPTY scenarios added; CI result pending |
| Parent termination and restart | SIGKILL/restart tests passed on Unix CI | SIGKILL/restart tests passed on native macOS CI | Job Object cleanup and ConPTY restart/no-replay scenarios added; CI result pending |
| No automatic replay | Write-ahead phases; uncertain outcomes remain unresolved | Same shared controller contract | Same shared controller contract; native restart test pending |
| Reference device | Arch live-device gate **UNVERIFIED** | Not a release reference device | Native Windows CI is pending; no live installed-Codex qualification is recorded |

## Qualification snapshot

| Platform/configuration | Implementation | Runtime evidence | Release status |
| --- | --- | --- | --- |
| Linux x86_64, audited Codex 0.163.0 | Existing Unix journal and PTY controller; P038 deterministic persistence failpoints added | P037 Linux CI passed. P038 local Rust workspace, Clippy, and all 9 Unix PTY scenarios passed on Debian Linux | Implementation available; Arch reference-device release gate remains unverified |
| macOS x86_64 and ARM64, audited Codex 0.163.0 | Existing Unix journal and PTY controller | P037 native macOS CI passed. P038 regression CI must pass before qualification is refreshed | Implementation available; no additional macOS reference-device claim |
| Windows x86_64, audited Codex 0.163.0 | ACL-backed journal, `LockFileEx`, Job Object process cleanup, and ConPTY tests implemented | Windows-only journal, transport, and ConPTY test sources pass a reduced GNU-target Clippy check. Native Windows test run is pending; no live Codex runtime test is recorded | **Not yet release-qualified** |
| Other operating systems, filesystems, removable media, and network drives | No support claim | Not validated; Windows journal rejects non-fixed and non-local volumes | Unsupported |

The P037 CI history is recorded in [PROMPT037_MANIFEST.md](PROMPT037_MANIFEST.md). P038 native
workflow links and results will be recorded in the P038 manifest when available.

## Windows journal boundaries

Windows does not emulate POSIX modes. Journal directories and files receive an explicit protected
security descriptor owned by the current user. The backend accepts only the current user's exact
full-access ACE on private entries, rejects inherited or additional ACEs there, checks that parent
directories do not grant write or delete access to other local principals, rejects hard-linked
journal files, and opens entries without following reparse points. It uses local fixed drives and
rejects network, removable, and other non-fixed storage. A lock file is metadata-only and contains
no recovery state.

The journal schema remains version 1 and is shared across the Unix and Windows backends. Writes
flush the temporary record before same-directory replacement. Failed or interrupted writes can
leave a stale `.journal.tmp`; a later locked write validates and overwrites that private temporary
file. The authoritative `journal.json` is never replaced by malformed or checksum-invalid content.

`ReplaceFileW` and write-through move operations provide the Windows replacement behavior requested
by this implementation. Process-crash tests establish restart behavior at injected persistence
boundaries. They do not prove survival of every kernel crash or loss of power: filesystem metadata,
device caches, controller firmware, and hardware power-loss behavior remain outside these tests.
The Unix file and directory synchronization barriers also do not constitute a hardware-level power
failure qualification.

## Terminal and process ownership

The interactive `codex-meter` process uses the terminal supplied by its host. On Windows the host
typically supplies a ConPTY; Codex Meter does not create a second pseudo console to run the App
Server. Its App Server connection remains a bounded JSON-RPC stdio transport. The transport places
the Codex executable in a Windows Job Object with kill-on-close enabled so App Server descendants
are terminated when Codex Meter exits or its process handle closes. Assignment failures fail
startup closed. The native ConPTY tests host the compiled TUI inside a pseudo console and exercise
approval cancellation, interruption, parent termination, restart, and no replay with a synthetic
App Server.

When the process or App Server can no longer prove whether a mutation ran, the controller keeps the
record unresolved. It may reconcile an exact known thread and turn through read-only server calls;
it never submits, resumes, approves, or interrupts automatically during recovery.
