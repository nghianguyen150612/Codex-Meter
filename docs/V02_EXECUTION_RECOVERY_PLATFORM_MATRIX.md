# Execution recovery platform matrix (P038)

This matrix separates implementation, synthetic runtime evidence, native CI, and live-device
qualification. The Codex App Server allowlist remains exactly codex_cli_rs/0.163.0. This matrix
does not broaden it.

## Capability matrix

| Capability | Linux | macOS | Windows |
| --- | --- | --- | --- |
| Interactive terminal | Unix PTY through the host terminal | Unix PTY through the host terminal | Crossterm console I/O hosted in a native ConPTY; the host owns the ConPTY |
| Private journal security | Current-user owned directory 0700; files 0600; no-follow component opens | Current-user owned directory 0700; files 0600; no-follow component opens | Protected DACL with only the current-user SID granted access; owner, DACL, ancestors, file type, hard-link count, reparse status, and local fixed-volume path checked |
| Atomic persistence | Same-directory rename, file and directory fsync | Same-directory rename, file and directory fsync | FlushFileBuffers, ReplaceFileW for replacement, MoveFileExW with write-through for initial creation |
| Interprocess locking | flock plus process mutex | flock plus process mutex | LockFileEx byte-range lock plus process mutex; five-second acquisition bound |
| Revision compare-and-swap | Shared journal CAS under lock | Shared journal CAS under lock | Shared journal CAS under lock |
| Approval and interruption | Unix PTY synthetic-server scenarios passed | Native macOS CI scenarios passed | Native ConPTY synthetic-server scenarios passed |
| Parent termination and restart | SIGKILL/restart tests passed | SIGKILL/restart tests passed in native CI | Job Object cleanup and ConPTY restart/no-replay scenarios passed in native CI |
| No automatic replay | Write-ahead phases; uncertain outcomes stay unresolved | Same shared controller contract | Same shared contract; native synthetic parent-crash/restart tests passed |
| Reference-device status | Arch live reference-device gate **UNVERIFIED / RELEASE BLOCKER** | Not the designated reference device | Native security CI passed; live installed-Codex gate **UNVERIFIED / RELEASE BLOCKER** |

## Qualification snapshot

| Platform/configuration | Implementation and runtime evidence | Release status |
| --- | --- | --- |
| Linux x86_64 | Unix journal, deterministic persistence failure injection, and all nine Unix PTY scenarios passed in [final CI run 39888358799](https://github.com/nghianguyen150612/Codex-Meter/actions/runs/39888358799) on SHA 310e6a83. The session host was Debian 13.6 in an isolated overlay environment. | Implementation and Linux CI pass; no Arch reference qualification. |
| macOS x86_64 and ARM64 | Native macOS platform jobs passed in [final CI run 39888358799](https://github.com/nghianguyen150612/Codex-Meter/actions/runs/39888358799) on SHA 310e6a83. | Implementation and CI pass; not a substitute for Arch qualification. |
| Windows x86_64 | Native journal tests and four synthetic-server ConPTY tests passed in [final CI run 39888358799](https://github.com/nghianguyen150612/Codex-Meter/actions/runs/39888358799) on SHA 310e6a83. Release bundles passed in [run 39888358815](https://github.com/nghianguyen150612/Codex-Meter/actions/runs/39888358815). | Native security and synthetic lifecycle pass. Live installed Codex and authentication qualification remain unverified. |
| Arch Linux physical x86_64 reference device | Not available in this session. A reproducible guard and test runner are in scripts/p038a-qualify-arch.sh. | **UNVERIFIED / RELEASE BLOCKER** |
| Other operating systems, filesystems, removable media, and network drives | No support claim. Windows rejects non-fixed and non-local storage. | Unsupported or unvalidated. |

Run details and final gates are in [PROMPT038A_MANIFEST.md](PROMPT038A_MANIFEST.md). The cited
implementation run 39887178698 passed on earlier code SHA 899447a; final CI and release validation
on the original PR head 310e6a83 passed as runs 39888358799 and 39888358815. The full results were
checked against GitHub run metadata and job summaries.

## Windows journal boundaries

Windows does not emulate POSIX modes. Journal directories and files receive a protected security
descriptor owned by the current user. The backend accepts only the current user's exact full-access
ACE on private entries, rejects inherited or additional ACEs there, checks parent directories for
write or delete access granted to other local principals, rejects hard-linked journal files, and
opens entries without following reparse points. It uses local fixed drives and rejects network,
removable, and other non-fixed storage. A lock file is metadata-only.

The schema remains version 1 across Unix and Windows. Writes flush the temporary record before
same-directory replacement. Failed or interrupted writes can leave a stale .journal.tmp; a later
locked write validates and overwrites that private temporary file. The authoritative journal.json
is not replaced by malformed or checksum-invalid content.

## Durability boundary

Successful process-crash recovery is covered by injected persistence failures and process
termination/restart tests. Filesystem write ordering is requested with Unix file and directory
syncs or Windows FlushFileBuffers and write-through replacement. Atomic namespace replacement is
implemented with Unix rename or Windows ReplaceFileW/write-through MoveFileExW and is tested.

These properties do not prove survival of every operating-system crash, filesystem metadata loss,
storage-controller cache loss, or hardware power loss. Arch filesystem durability was not
qualified here. OS-crash and hardware power-loss durability remain **UNVERIFIED**; power-loss
testing is not an additional release prerequisite under this P038 task.

## Terminal, process ownership, and recovery

The host supplies the interactive terminal. On Windows, Codex Meter does not create a second
pseudo-console to run the App Server. Its transport uses bounded JSON-RPC over stdio and places the
Codex executable in a Job Object with kill-on-close enabled. Assignment failures fail startup
closed. Native ConPTY tests host the compiled TUI and exercise approval cancellation, interruption,
parent termination, restart, and no replay with a synthetic App Server.

When the process cannot establish whether a mutation ran, the record remains unresolved. Recovery
can inspect only an exact known thread and turn through read-only calls; it never submits, resumes,
approves, or interrupts automatically.
