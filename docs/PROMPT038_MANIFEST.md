# PROMPT038 implementation manifest

## Status

- Status: **PARTIAL**. Native Windows CI now passes the journal and synthetic ConPTY lifecycle
  suites. Arch live-device qualification remains a release blocker, and no live installed Codex
  Windows execution qualification is recorded.
- Evidence snapshot: 2026-10-23.
- Implementation branch: `feat/p038-windows-execution-reliability`.
- Baseline: `352f4e0c499872f31205d75680172b8c1c8341a0`, the existing `origin/main` merge commit for
  P037 PR #32.
- P037 commit under review: `a1d7ec598f539a324eec434e27335035daf5f716`.
- P037 PR: [#32](https://github.com/nghianguyen150612/Codex-Meter/pull/32). It is already merged
  upstream. P037's CI jobs passed, but its own manifest still recorded the Arch reference gate as
  pending and Windows ACL storage/ConPTY as unimplemented. Under the requested preflight criteria,
  P037 was not eligible for merge when reviewed. No rollback was performed; P038 starts from the
  already merged `origin/main` commit and does not duplicate P037's Unix journal or no-replay
  controller.
- CI baseline: P037 head workflow runs [39779412978](https://github.com/nghianguyen150612/Codex-Meter/actions/runs/39779412978),
  [39779404800](https://github.com/nghianguyen150612/Codex-Meter/actions/runs/39779404800), and
  [39779413032](https://github.com/nghianguyen150612/Codex-Meter/actions/runs/39779413032) passed.
  Merge-head CI [39785737834](https://github.com/nghianguyen150612/Codex-Meter/actions/runs/39785737834)
  and release validation [39785737845](https://github.com/nghianguyen150612/Codex-Meter/actions/runs/39785737845)
  passed. These results did not satisfy the live Arch device gate.
- P038 implementation-head run [39887178698](https://github.com/nghianguyen150612/Codex-Meter/actions/runs/39887178698)
  passed Rust quality, Linux, native Windows, macOS x86_64/ARM64, contract, Python quality, and the
  other installed-runtime jobs. The overall run is still in progress because its macOS 3.13
  installed-runtime smoke job is queued; that same smoke passed in the immediately prior P038 run
  [39886904765](https://github.com/nghianguyen150612/Codex-Meter/actions/runs/39886904765), which
  only failed Rust formatting on the assertion layout corrected in `899447a`. Release candidate
  validation passed in [run 39887178747](https://github.com/nghianguyen150612/Codex-Meter/actions/runs/39887178747).

## Implementation plan

1. Reuse the schema-version-1 recovery journal and add a native Windows security and persistence
   backend.
2. Add Windows kernel locking and retain the shared revision compare-and-swap rules.
3. Bound App Server process-tree lifetime with a Windows Job Object and exercise the TUI under a
   native ConPTY.
4. Share deterministic persistence failpoints across Unix and Windows and cover ACL, replacement,
   conflict, concurrency, approval, interruption, crash, restart, and no-replay behavior.
5. Update compatibility evidence and document limits without claiming Arch or power-loss
   qualification absent device evidence.

## Implementation summary

The Windows journal backend in `execution_recovery/platform.rs` derives the current user's SID from
the process token and applies a protected DACL granting access only to that SID. It rejects non-
absolute, non-local, non-fixed-volume paths, reparse points, non-regular files, owner mismatches,
and private entries with inherited or additional ACEs. It reads and writes the existing version-1
envelope and checksum, caps journal input at 64 KiB, and preserves corrupt records.

Writes use a same-directory `.journal.tmp`, flush file contents with `FlushFileBuffers`, and use
`ReplaceFileW` or a write-through `MoveFileExW` for atomic namespace replacement. Windows
coordination uses a process mutex plus `LockFileEx` with a bounded acquisition wait; kernel locks
release when handles close. Record revisions continue to use P037's compare-and-swap contract.
The backend does not emulate POSIX modes and has no permissive fallback.

The Windows App Server transport assigns the spawned Codex process to a Job Object configured with
`JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`. Assignment failure aborts startup. Closing or terminating the
transport kills the process tree. The user-facing TUI continues to attach to the host terminal;
native Windows tests provide a ConPTY and a synthetic `codex.exe` fixture.

P037's write-ahead controller remains the no-replay authority. An uncertain mutation stays
unresolved and can only be reconciled against an exact known thread/turn. The journal does not
contain prompt, response, source, command, approval, transcript, credential, environment, email, or
remote-URL content. Windows flush and write-through calls improve process-crash persistence, but
the tests do not claim survival of every operating-system crash, storage-controller cache, or
power-loss scenario.

## Validation record

| Gate | Status | Evidence |
| --- | --- | --- |
| P037 preflight and baseline | **PASS WITH BLOCKERS** | PR #32 already merged at baseline `352f4e0`; P037 CI passed, Arch gate remained pending, and Windows ACL/ConPTY work was absent |
| Linux Rust journal unit suite | **PASS** | Included in 463 passing Rust library tests; deterministic write-boundary failures passed |
| Linux Unix PTY suite | **PASS** | All 9 existing P037 lifecycle, approval, interruption, crash, restart, and no-replay scenarios passed |
| macOS x86_64 and ARM64 | **PASS** | P038 native regression suites passed on macOS 15 Intel and ARM64 in [CI run 39887178698](https://github.com/nghianguyen150612/Codex-Meter/actions/runs/39887178698) |
| Windows ACL, replacement, CAS, and concurrency tests | **PASS** | 461 native Windows library tests passed, including owner ACL checks, explicit denial of current-user access, hard-link rejection, replacement, revision conflicts, concurrent processes, and interrupted-lock recovery in [CI run 39887178698](https://github.com/nghianguyen150612/Codex-Meter/actions/runs/39887178698) |
| Windows ConPTY approval, interruption, parent crash, restart, and no-replay tests | **PASS** | Four native ConPTY integration tests passed with a synthetic local App Server in [CI run 39887178698](https://github.com/nghianguyen150612/Codex-Meter/actions/runs/39887178698); no live installed Codex execution is recorded |
| Deterministic persistence fault injection | **PASS** | Failures injected before/during write, before/after flush, before/after replacement, during lock acquisition, revision CAS, cleanup, and recovery; malformed/truncated/corrupt, stale temporary, conflicting revision, inaccessible ACL, and unsafe journal records fail closed |
| Overall CI workflow | **BLOCKED ON QUEUED JOB** | All required Rust platform, quality, contract, and Python jobs passed in [run 39887178698](https://github.com/nghianguyen150612/Codex-Meter/actions/runs/39887178698); its macOS 3.13 installed-runtime smoke remains queued, with the same smoke passed in prior run 39886904765 |
| Windows-only source and test type-check | **PASS** | Reduced target harness passed `cargo clippy --all-targets --target x86_64-pc-windows-gnu -- -D warnings`; includes Windows journal, transport Job Object, and ConPTY test sources |
| Full Windows target build | **PASS IN NATIVE CI** | Native `windows-latest` workspace build and all-target tests passed in [CI run 39887178698](https://github.com/nghianguyen150612/Codex-Meter/actions/runs/39887178698). Linux cross-build attempts remain blocked by missing Windows C toolchains for bundled SQLite |
| Arch Linux reference-device gate | **UNVERIFIED / RELEASE BLOCKER** | This environment is Debian Linux; no live Arch reference device was available |
| Filesystem power-loss durability | **UNVERIFIED** | Process crash and injected boundary tests are not hardware power-loss testing |
| Codex runtime version allowlist | **UNCHANGED** | Only audited `codex_cli_rs/0.163.0` remains allowlisted |

### Environment record

The implementation environment is Debian Linux x86_64, not Arch Linux. No live Arch hardware
qualification is claimed. Local validation commands and their results:

- `cargo fmt --manifest-path rust/Cargo.toml --all -- --check` — **PASS**.
- `cargo clippy --manifest-path rust/Cargo.toml --workspace --all-targets --all-features -- -D warnings` — **PASS**.
- `cargo test --manifest-path rust/Cargo.toml --workspace --all-targets` — **PASS**: 463 library, 4 analytics CLI, 19 sync CLI, 5 TUI CLI, and 9 Unix PTY tests.
- `cargo test --manifest-path rust/Cargo.toml --workspace --all-targets --all-features` — **PASS**: 461 library, 2 analytics CLI, 19 sync CLI, 5 TUI CLI, and 9 Unix PTY tests.
- Windows reduced target harness `cargo clippy --all-targets --target x86_64-pc-windows-gnu -- -D warnings` — **PASS** for Windows recovery, transport, and ConPTY test sources. Native Windows build and runtime test evidence is in CI run 39887178698.
- `cargo check --manifest-path rust/Cargo.toml -p codex-meter --target x86_64-pc-windows-msvc` — **BLOCKED** before the project crate because the Linux environment has no `lib.exe` for bundled SQLite.
- `cargo check --manifest-path rust/Cargo.toml -p codex-meter --target x86_64-pc-windows-gnu` — **BLOCKED** before the project crate because `x86_64-w64-mingw32-gcc` is unavailable for bundled SQLite.

The host reports Debian GNU/Linux x86_64 with kernel `6.18.55`; the workspace filesystem is an
overlay. The available Rust toolchain is stable `1.99.1`. Native Windows CI is recorded above, but
there is no live installed Codex Windows execution evidence. The Arch live-device gate remains
outstanding.

## Platform matrix

See [V02_EXECUTION_RECOVERY_PLATFORM_MATRIX.md](V02_EXECUTION_RECOVERY_PLATFORM_MATRIX.md) for
Linux, macOS, and Windows capability details, release qualification status, and the limits of
process-crash evidence.

## Commit, pull request, and merge

P038 implementation head for this evidence snapshot: `899447a9584efed259358a3c7a870083e640b338`.
The evidence update is on the existing `feat/p038-windows-execution-reliability` branch. PR
[#33](https://github.com/nghianguyen150612/Codex-Meter/pull/33) is open as a draft and is not merged.
P038 remains **PARTIAL** because Arch live-device qualification is a release blocker and live
installed Codex Windows execution has not been qualified.
