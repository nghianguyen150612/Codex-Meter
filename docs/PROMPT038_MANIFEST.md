# PROMPT038 implementation manifest

## Status

- Status: **PARTIAL**. Cross-platform implementation and CI gates pass. The mandatory live
  installed-Codex Windows gate and native Arch reference-device gate remain unverified.
- Evidence snapshot: 2026-10-23.
- Branch: feat/p038-windows-execution-reliability.
- PR: [#33](https://github.com/nghianguyen150612/Codex-Meter/pull/33), open as a draft and not
  merged.
- P038A qualification record: [PROMPT038A_MANIFEST.md](PROMPT038A_MANIFEST.md).

## Baseline and source commits

- P038 starting commit / original PR head: 310e6a83f2b3128477c05bbb28805f73444874d2.
- origin/main at preflight: 352f4e0c499872f31205d75680172b8c1c8341a0, the P037 PR #32 merge.
- PR #33 base SHA at preflight: 352f4e0c499872f31205d75680172b8c1c8341a0.
- The final CI and release-validation evidence below is tied to the original PR head. P038A
  documentation and procedure changes have their own final commit and checks in the P038A record.

P037's journal and no-replay controller remain the shared recovery authority. P038 adds the
Windows ACL-backed storage adapter, Windows locking and replacement, process-tree cleanup, native
ConPTY tests, and deterministic persistence fault injection. It does not change the audited runtime
allowlist or introduce P039 behavior.

## Implementation summary

The Windows journal backend in
rust/crates/codex-meter/src/codex_app_server/execution_recovery/platform.rs derives the current
user SID from the process token and applies a protected DACL granting access only to that SID. It
rejects non-absolute, non-local, non-fixed-volume paths, reparse points, non-regular files, owner
mismatches, inherited or additional ACEs on private entries, unsafe ancestors, and hard-linked
journals. It reads and writes the shared version-1 envelope and checksum, caps journal input at
64 KiB, and preserves corrupt records.

Writes use a same-directory temporary file, FlushFileBuffers, ReplaceFileW or a write-through
MoveFileExW, and LockFileEx with a bounded acquisition wait. The process-local mutex covers threads
in one process; kernel locks release when their handles close. Revision updates keep the shared
compare-and-swap contract.

The Windows App Server process is assigned to a Job Object configured with kill-on-close.
Assignment failure aborts startup. Native ConPTY tests use a synthetic local App Server and do not
send a request to an installed Codex account.

The journal stores execution identifiers, known thread and turn identifiers, mutation intent,
state, and timestamps. It never stores prompts, responses, source, command content, approvals,
transcripts, credentials, environment values, email, or remote URLs. Unknown outcomes remain
unresolved and are never automatically replayed. Reconciliation requires an exact known thread and
turn; missing identity or server evidence stays unknown.

## CI and release-validation evidence

The three cited workflow results were checked against GitHub's workflow-run records:

| Workflow | Result | Run head SHA | Evidence |
| --- | --- | --- | --- |
| Implementation CI, run 39887178698 | **PASS** | 899447a9584efed259358a3c7a870083e640b338 | [Run](https://github.com/nghianguyen150612/Codex-Meter/actions/runs/39887178698). Earlier successful implementation head; not the final PR SHA. |
| Final CI, run 39888358799 | **PASS** | 310e6a83f2b3128477c05bbb28805f73444874d2 | [Run](https://github.com/nghianguyen150612/Codex-Meter/actions/runs/39888358799). All 16 jobs completed successfully, including Linux, Windows, macOS Intel and ARM64, Rust quality, Python quality, contracts, and installed-runtime cells. |
| Release candidate validation, run 39888358815 | **PASS** | 310e6a83f2b3128477c05bbb28805f73444874d2 | [Run](https://github.com/nghianguyen150612/Codex-Meter/actions/runs/39888358815). All six jobs passed, including Linux, Windows, and both macOS bundles and aggregate verification. |

The successful earlier implementation run 39887178698 is not the final source SHA. The final CI and
release validation runs 39888358799 and 39888358815 are the applicable successful results for
310e6a83. Their run records and job results were checked before use.

## P038 implementation evidence

| Check | Status | Evidence |
| --- | --- | --- |
| Unix recovery journal and persistence fault injection | **PASS** | Linux workspace tests cover permissions, corruption, atomic replacement, concurrent locking, revision CAS, deterministic write/flush/replace/lock/cleanup/recovery failures, and stale temporary files. |
| Linux Unix PTY lifecycle | **PASS** | Final Ubuntu CI counted 463 library, 4 analytics CLI, 19 sync CLI, 5 TUI CLI, and 9 PTY tests, all passing. The PTY scenarios cover approval, cancellation, interruption, process kill, restart, reconciliation, and no replay. |
| Windows journal security and persistence | **PASS** | Final native Windows CI counted 461 library, 4 analytics CLI, 17 sync CLI, and 5 TUI CLI tests, all passing. Journal tests include owner/DACL checks, additional ACE and ancestor rejection, reparse and hard-link rejection, local fixed-volume enforcement, inaccessible journal handling, replacement, LockFileEx, and revision conflicts. |
| Windows ConPTY and process-tree cleanup | **PASS, SYNTHETIC SERVER** | Four native ConPTY tests passed in the final CI run. They cover approval cancellation, interruption, parent termination, restart, Job Object cleanup, and no replay without contacting live Codex. |
| macOS Intel and ARM64 | **PASS** | Native macOS test jobs passed in the final CI run. |
| Rust formatting, Clippy, workspace tests | **PASS** | All final CI quality and platform jobs passed at 310e6a83. |
| All-features Rust workspace tests | **PASS on 899447a** | The pre-existing local qualification record reports 461 library, 2 analytics CLI, 19 sync CLI, 5 TUI CLI, and 9 Unix PTY tests passing. The subsequent 310e6a83 change was documentation-only; final CI also passed formatting, all-features Clippy, and default workspace tests at 310e6a83. |
| Python quality and contract tests | **PASS** | Final CI Python quality and contract jobs passed at 310e6a83. |
| Installed-runtime smoke matrix | **PASS** | All nine Python 3.11–3.13 OS cells passed in final CI. These analytics package smokes do not qualify the live Codex App Server gate. |
| Release candidate bundles | **PASS** | Linux x86_64, Windows x86_64, macOS x86_64, macOS ARM64, shared wheel, and aggregate bundle validation passed in run 39888358815. |
| Live installed Codex on Windows | **UNVERIFIED / RELEASE BLOCKER** | No native Windows execution environment or live installed audited runtime was available in this session. Synthetic ConPTY results do not satisfy this gate. |
| Native Arch reference device | **UNVERIFIED / RELEASE BLOCKER** | This session ran in a Debian 13.6 x86_64 isolated environment with an overlay filesystem, not on the reference Arch device. No remote device was accessed. |
| Hardware power-loss durability | **UNVERIFIED; NOT A RELEASE PREREQUISITE HERE** | Process-crash tests and filesystem barriers do not prove survival of OS crash, device-cache loss, or hardware power loss. |

## Local qualification environment

The session environment was classified as an isolated Debian GNU/Linux 13.6 x86_64 container,
kernel 6.18.55. The workspace root used an overlay filesystem with read-write and relatime options;
the root overlay mount had index=off and metacopy=off. Standard input was not attached to a
terminal. The Codex CLI reported codex-cli 0.159.0-alpha.3. rustc, rustup, and cargo were not
available on PATH. This is neither Arch reference-device evidence nor native Windows evidence.

No user device was accessed. No Codex authentication state or credentials were inspected. No live
task or external prompt was sent. The local CLI display version does not establish the App Server
userAgent used by the P038 runtime allowlist.

## Durability boundary

| Property | Evidence boundary |
| --- | --- |
| Process-crash recovery | Covered by synthetic process termination, restart, and deterministic persistence-fault tests. |
| Filesystem write ordering | Unix code issues file and directory synchronization; Windows flushes the temporary file and requests write-through replacement. Device behavior was not qualified on the Arch reference filesystem. |
| Atomic namespace replacement | Same-directory Unix rename and Windows ReplaceFileW/write-through MoveFileExW are implemented and tested. Atomic namespace replacement does not imply every storage layer is durable after sudden power loss. |
| OS crash durability | **UNVERIFIED** across filesystem, kernel, and storage-controller failure modes. |
| Hardware power-loss durability | **UNVERIFIED**; no physical power-loss experiment was performed or required by this task. |

## Release gates

| Gate | Result | Evidence or remaining action |
| --- | --- | --- |
| G1 — Baseline | **PASS** | PR #33, branch, starting SHA 310e6a83, and base SHA 352f4e0 were verified. |
| G2 — Cross-platform CI | **PASS at 310e6a83** | Required CI passed on 310e6a83. The current PR head must also have its required checks green before merge; no functional result is inferred from the documentation/procedure update. |
| G3 — Windows native security | **PASS** | Native Windows journal/security tests passed in final CI run 39888358799. |
| G4 — Windows live runtime | **UNVERIFIED / BLOCKED** | Run the Windows procedure on native Windows with the actual installed App Server userAgent exactly codex_cli_rs/0.163.0, existing authentication verified locally, and the bounded live scenarios recorded. |
| G5 — Arch reference | **UNVERIFIED / BLOCKED** | Run scripts/p038a-qualify-arch.sh from a clean checkout on the physical Arch x86_64 reference device and return its sanitized output. |
| G6 — No replay | **PASS** | Unix PTY and Windows synthetic ConPTY restart tests preserve uncertain state and show no second turn/start. Exact thread/turn matching is required for reconciliation. |
| G7 — Evidence | **PASS after P038A commit and PR-body update** | P038A manifest, platform matrix, recovery docs, and qualification procedures are synchronized; the PR description will cite this evidence and the remaining blockers. |
| G8 — Merge readiness | **BLOCKED** | G4 and G5 are mandatory and remain unverified. PR #33 must stay draft and unmerged. |

P038 is still **PARTIAL**. Do not mark it complete, mark PR #33 ready, or merge while G4 or G5 is
unverified. The P038A record gives the exact manual procedures and sanitized evidence format.
