# PROMPT037 implementation manifest

## Status and scope

- Status: **PARTIAL** while Windows journal privacy guarantees, crash injection at every filesystem
  boundary, and the Arch reference-device gate remain outstanding.
- Verified starting `origin/main`: `18225c724813d85eaa76324e3f3674d116eee9a7` (P036 merge commit,
  PR #31).
- Feature branch: `feat/p037-execution-reliability`.
- Proposed PR: `Add durable execution recovery and end-to-end reliability validation`.
- No changes are made directly to `main`; v0.1.0 and P031–P036 merge history are preserved. P037
  does not publish v0.2.0 or begin P038.
- Scope is a private execution journal, integration into the existing P036 controller, startup
  recovery UX, read-only exact-identity reconciliation, Unix PTY subprocess tests, compatibility
  review, and documentation. Quota forecasting, Savings Goals, task-token attribution, plugin
  packaging, Codex Cloud, and cloud workspaces remain out of scope.

## Implementation summary

`execution_recovery.rs` contains the bounded schema-version-1 JSON journal and Unix storage adapter.
The record contains an opaque execution ID, origin, optional validated thread/turn IDs, mutation
intent, execution phase, confirmation state, last authoritative status, timestamps, revision,
warning acknowledgement, and recovery-required flag. A SHA-256 checksum detects corruption.
Unknown schema and malformed or truncated data fail closed and are preserved.

The Unix directory is private (`0700`), files are owner-only (`0600`), path components and files
are opened without following symlinks, writes use a fixed bounded temp file plus `fsync`, atomic
rename, and directory `fsync`. Writers serialize with a process-local mutex and OS `flock`, then
use revision compare-and-swap to reject stale state.
The store caps the journal at 32 records / 64 KiB, prunes only safe pre-mutation or terminal
records, and retains all unresolved records. Concurrent stale updates fail rather than overwrite.

Windows derives the intended `%LOCALAPPDATA%` path but the current storage adapter explicitly
returns `UnsupportedPlatform`. It does not write a fallback file. This is fail-closed behavior, not
Windows support; execution cannot start on Windows until private ACL and safe-replacement behavior
are implemented and tested.

The existing P036 `InteractiveExecution` controller performs writes before `thread/start`,
`thread/resume`, `turn/start`, approval responses, and `turn/interrupt`. For each ambiguous request,
it persists intent and then a may-have-been-sent phase before transport transmission. It persists
validated thread/turn identities before the next mutation. Terminal state is stored before the TUI
reports it as final. An uncertain error, journal failure after possible acceptance, or critical
event loss leaves an unresolved record. No mutation is automatically retried.

Startup opens and validates the journal before the first interactive draw, then shows the recovery
view for unresolved records. This prevents startup latency from leaving a keyboard window before
the warning appears. Reconciliation is an explicit user action, validates the exact known thread
and turn through P036 read-only APIs, and never
submits, resumes, approves, or interrupts automatically. A known thread with unknown turn identity
and an unknown thread remain outcome-unknown; this build does not guess from turn position,
timestamp, workspace, or prompt content. Warning acknowledgement is stored separately and does
not resolve or delete the execution record.

Approvals are not reconstructed after a crash. Their request contents are never journaled. An
explicit recovery interrupt first reads the exact known turn and sends one newly journaled
interrupt only when the user asks and Codex reports that turn still running; acknowledgement is
kept distinct from a terminal interrupted state.

## PTY and recovery evidence

`pty_execution_recovery.rs` launches the compiled binary on a Unix PTY, isolates HOME, XDG data, and
workspace paths in a temporary directory, and injects a synthetic `codex app-server` executable
only through that child process's `PATH`. The fake verifies exact prompt bytes and synthetic request
parameters and counts mutation methods. The scenarios cover:

- New-thread submission, observed running state, completed event, and clean terminal restoration.
- Explicit allow-once and default-deny approval responses.
- Interrupt acknowledgement remaining nonterminal until Codex reports interruption.
- Connection loss, exact known-turn reconciliation, and explicit interrupt after a fresh running
  observation.
- SIGKILL during active execution, restart detection, read-only reconciliation, explicit
  interrupt acknowledgement, and no second `turn/start`.
- SIGKILL after possible `thread/start` and `turn/start` transmission but before acknowledgement,
  preserving unknown identities and proving no replay from request counts.
- Dirty Composer exit, `NO_COLOR`, `LC_ALL=C` Unicode input, narrow terminal resize, alternate
  screen and cursor restoration on normal exits.

SIGKILL bypasses cleanup. Crash tests restore their PTY from the test process before restart and do
not claim that the killed process restored terminal settings. The PTY framework uses Unix
`openpty`; Windows ConPTY remains unverified. Native macOS x86_64 and ARM64 CI passed all nine PTY
scenarios, including SIGKILL/restart and terminal restoration after the test harness resets its PTY.

Journal unit tests cover privacy canaries, checksums/truncation, schema mismatch, process-store
reopen after possible submission, stale compare-and-swap updates, warning acknowledgement without
resolution, concurrency, symlink/public-permission rejection, bounded storage, and preservation of
unresolved records. Full crash injection at each filesystem instruction is not yet implemented;
the documented crash table defines the expected recovery semantics.

## Runtime and device audit

Only `codex_cli_rs/0.163.0` remains allowlisted. P037 compared targeted checked-in protocol schemas
from upstream stable tag `rust-v0.162.1` (commit
`092d3acd6bec3e3a14bdc7e7a2810ab628ab759d`) with the 0.163.0 baseline. Core approval and lifecycle
schemas match, while client request, notification, resume input, turn input, and thread-read schemas
have differences. No 0.162.1 runtime/fake-server/live-read-only evidence exists, so it is not
enabled. See [V02_CODEX_COMPATIBILITY_MATRIX.md](V02_CODEX_COMPATIBILITY_MATRIX.md).

The implementation environment reports `codex-cli 0.163.0`; `codex app-server --help` succeeds.
That environment is not the reference Arch desktop. No personal session was resumed and no paid
task was submitted. The Arch gate remains **Pending**.

## Validation record

| Gate | Status |
| --- | --- |
| Starting `origin/main` | Verified at `18225c724813d85eaa76324e3f3674d116eee9a` before branch creation |
| Unix journal unit tests | Passed in default and all-features Rust workspace suites |
| PTY execution, approvals, interrupt, connection loss | 9 subprocess PTY scenarios passed in default and all-features Rust suites |
| PTY SIGKILL / restart after active execution | Passed; request counts confirmed no replay |
| PTY SIGKILL after possible thread/turn start | Both boundaries passed; request counts confirmed no replay and identities remain unknown |
| `cargo fmt --check`, Clippy, default/all-feature Rust tests, build | Passed on Linux x86_64; counts below |
| Rust default workspace suite | Passed: 462 library, 4 analytics CLI, 19 sync CLI, 5 TUI CLI, and 9 PTY tests |
| Rust all-features workspace suite | Passed: 460 library, 2 analytics CLI, 19 sync CLI, 5 TUI CLI, and 9 PTY tests |
| Python tests and tooling | Passed: 638 pytest tests; Ruff lint and format; dependency lock verification; 6 schemas, 16 positive and 5 negative contract fixtures |
| P029 installed-runtime smoke | Passed: reproducible wheel, isolated install, relocation, source mode, cwd/path, deterministic output, concurrency, and read-only SQLite checks |
| P030 local release candidate | Passed: clean Linux x86_64 candidate packaged from source `a10de413ba5bf0cac409cd92a2738fa3ac408b2f`; extracted-candidate smoke verified installed worker, estimates, deterministic JSON, version compatibility, and read-only SQLite |
| P030 release-candidate CI | Passed on P037 head `674cb27a0a9c68bc52698a074d203d64f10bd97a` in run `39778573401`; Linux x86_64, macOS x86_64, macOS ARM64, and Windows x86_64 artifact jobs succeeded |
| P031–P036 regressions | Passed through retained Rust workspace and Python regression suites; no separate per-milestone executable smoke scripts exist |
| Linux x86_64 | Local implementation environment; Rust, Python, P029, and journal/PTY suites passed |
| macOS x86_64 and ARM64 | Native Rust CI passed on head `674cb27`; journal, PTY, crash/restart, and terminal-restoration tests passed on both architectures |
| Windows x86_64 | Rust platform CI passed on head `674cb27`; journal execution deliberately fails closed until ACL-backed storage is implemented |
| Arch live read-only validation | **Pending; reference device unavailable** |
| GitHub Actions CI | Passed on head `674cb27` in run `39778573359`: Rust quality, Python quality, contracts, Linux/macOS/Windows Rust platform tests, and installed-runtime matrix |
| Merge / release | Not performed; v0.2.0 release is not authorized by this work |

The implementation runtime check was run in a Linux x86_64 workspace; it does not substitute for
the reference Arch desktop. Native macOS CI supports the Unix journal and PTY implementation claims,
but does not satisfy the live Arch device gate. Windows storage and ConPTY remain unsupported.

## Consolidated P031–P037 device gates

| Milestone | Reference Arch evidence | Status |
| --- | --- | --- |
| P031 | Actual rollout compatibility, sync, and replay behavior | Pending |
| P032 | Real token/history/quota reporting | Pending |
| P033 | Live sessions, projects, and models | Pending |
| P034 | Interactive TUI, resize, and terminal restoration | Pending |
| P035 | Composer, model selection, and private drafts | Pending |
| P036 | Safe execution UI and exact App Server protocol compatibility | Pending |
| P037 | Recovery detection, restart behavior, and PTY operation | Pending |

Controlled fixtures and CI can support the implementation gates but do not satisfy these live-device
checks.

## P038 handoff

Complete Windows ACL-backed journal storage and Windows terminal tests, execute the Arch read-only
validation, and expand crash injection and cross-platform process-restart coverage. Any later
task-token attribution or forecasting work needs separately verified telemetry identity rules and
must not treat one execution, thread, or terminal event as a quota delta. Do not add quota
forecasting, Savings Goals, plugin packaging, or cloud integration to P037.
