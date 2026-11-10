# P038A-FIX defect resolution

## Status and environment

Local engineering validation is **PASS**; final cross-platform CI is pending. P038 release remains **BLOCKED** by G4 and G5.
Baseline: `492e114c8968aa7a77973312ce337277f9052403`; base/main:
`352f4e0c499872f31205d75680172b8c1c8341a0`. PR #33 remains Draft.
No P039 work, runtime allowlist changes, journal security changes, or live tasks are included.

The implementation environment is managed cloud Debian 13 x86_64, Linux 6.18.57,
overlay filesystem, Rust stable 1.99.1, x86_64-unknown-linux-gnu. It is neither the
physical Arch device nor native Windows. No remote physical device was available.

## Original physical failure and reproduction

The operator reported Arch Linux 7.2.9-zen1-1-zen, physical x86_64, btrfs with
compress=zstd:3, Rust 1.98.1, clean baseline 492e114. Journal: 10/10; PTY: 8/9;
workspace library: 463 default / 461 all-features. The Unicode execution test timed
out after resizing from 120x40 to 16x8, expecting `Resize terminal`.
The full output did not contain the expected word; periodic redraw did not help.
These are operator-supplied results, not tests performed by this cloud session.

The unchanged baseline harness reproduced the same failure inside an outer interactive
PTY on cloud Debian using `script`, with the outer dimensions fixed at 120x40:

```sh
script -q -e -c 'stty cols 120 rows 40; cargo test --manifest-path rust/Cargo.toml -p codex-meter --test pty_execution_recovery compiled_tui_executes_unicode -- --test-threads=1' /dev/null
```

## Root cause and correction

The harness called `process_group(0)`, which changes the child's process group but
retains its session and inherited controlling terminal. Its stdio points at the
new slave PTY, but `/dev/tty` still names the invoking terminal. Crossterm 0.28.1
`terminal::sys::unix::window_size` opens `/dev/tty` first and falls back to stdout
only when that fails. Thus TIOCSWINSZ on the harness master changes the slave to
16x8, while the child still queries the outer terminal's 120x40 size. Explicit
SIGWINCH and repeated draws cannot correct a query against the wrong terminal.
Ratatui consequently keeps a wide frame and the fallback never executes. This is
a harness defect, not an Arch-specific renderer defect or a Unicode locale defect.

The child now calls setsid and TIOCSCTTY on stdin before exec, using only
async-signal-safe syscalls. It verifies its foreground group from inside that
session. TIOCSWINSZ now naturally delivers SIGWINCH to the intended foreground
group; the manual signal is removed. Master and slave original descriptors are
close-on-exec. TIOCGWINSZ checks the slave dimensions after each resize.

The screen helper also had independent limitations: fixed 180x64 dimensions,
one-cell Unicode assumptions, incomplete erase/control handling, and no alternate
screen state. The Unix test-only `vt100` dependency replaces it. Output offsets
record resize boundaries so reconstruction processes bytes with the corresponding
terminal dimensions. It handles split UTF-8, cursor moves, differential updates,
erase, wrapping, and alternate-screen/cursor state. Assertions inspect the current
screen, not raw string presence. Raw bytes are used only for existing lifecycle
sequence checks and a sanitized diagnostic boolean.

At 16 columns the fallback correctly occupies three lines: `Resize terminal`,
`to view Codex`, `task execution`. A render regression asserts those exact rows
and recovery to the wide Running view. The compiled lifecycle checks 120x40,
16x8, fallback, return to 120x40 and Running before completion, Unicode prompt
bytes under LC_ALL=C, terminal mode/alternate-screen/cursor restoration, and exactly
one thread/start and turn/start. Existing approval, crash, reconciliation, unknown
outcome and no-replay coverage is retained. CI also executes the inherited-terminal
reproduction on Linux, and default plus all-features tests on all native platforms.

Failure diagnostics expose only expected static UI labels, byte counts, dimensions,
raw/screen presence, known overlay markers, child existence, and mutation counts.
They no longer dump terminal transcripts or fake state. The Arch runner extracts
only allowlisted PTY_DIAG facts and failed test names.

## App Server probe

An isolated installation of official `@openai/codex@0.164.0` was initialized with
an empty temporary CODEX_HOME, client name codex-meter and version 0.1.0. No auth
files were inspected and no task was submitted. All four fields were strings:
userAgent, codexHome, platformFamily, platformOs. Its user agent identity was
`codex-meter/0.164.0`, followed by a parenthesized platform suffix. The original
probe reproduced `Initialize response metadata was missing or malformed`.
Its safe-field regex rejects spaces and parentheses, conflating a valid user agent
with missing/malformed metadata. This establishes a probe defect; it does not
establish compatibility of 0.164.0 or prove the operator's exact full response.

The corrected probe validates required strings and execution metadata byte bounds,
discards codexHome, withholds the full user-agent suffix, and separately applies
execution.rs's audited identity predicate: codex_cli_rs/0.163.0, optionally followed
by a space, slash, or plus suffix. It rejects development/version lookalikes and
0.164.0. The isolated 0.164.0 now reports `unsupported_runtime` and FAIL. The
production allowlist is unchanged. CLI display version is never compatibility proof.
Tests cover missing executable/startup, transport EOF, invalid response, missing
fields/types, platform suffixes, audited and unsupported identities, initialized
notification, and sanitized output. No live task is used.

## Qualification runner corrections

Both runners require P038A_EXPECTED_COMMIT to equal the reviewed full HEAD SHA,
reject dirty trees, and retain filesystem, toolchain, command exits and test counts.
Arch now retains the probe exit separately; passing Rust tests cannot erase a
failed or unavailable probe. Temporary captures are removed on normal exit and
interrupt. Windows records the probe exit and compatibility result directly,
without a second divergent user-agent predicate. The Windows runner also stops assigning
to PowerShell 7's read-only automatic IsWindows variable; it uses nativeWindowsHost. Both procedures keep live
qualification UNVERIFIED even when synthetic tests and initialize pass.

## Remaining device actions

G4 requires native Windows with the audited installed runtime and separately
explicitly authorized bounded live scenarios. Synthetic ConPTY is separate evidence.
G5 requires the designated physical Arch device to retest this corrected commit,
then complete the existing live/read-only reference checks. The original physical
failure remains a failed historical observation; corrected physical results are
**UNVERIFIED**. An unsupported 0.164.0 runtime remains a separate compatibility blocker.
Do not upgrade the allowlist to make a device qualification pass.

From a clean checkout on the designated device, fetch PR #33, check out its reviewed
HEAD and set P038A_EXPECTED_COMMIT to that full SHA before running:

```sh
P038A_EXPECTED_COMMIT=<reviewed-full-SHA> bash scripts/p038a-qualify-arch.sh --confirm-reference-device
```

On native Windows PowerShell:

```powershell
$env:P038A_EXPECTED_COMMIT = "<reviewed-full-SHA>"
.\scripts\p038a-qualify-windows.ps1
```

Do not share raw transcripts, prompts, Codex home paths, credentials, SIDs, or journals.
PR #33 must remain Draft and unmerged while G4 or G5 is unverified.

## Local validation — 2026-11-10

| Check | Result |
| --- | --- |
| Rust fmt / all-targets all-features Clippy -D warnings | PASS |
| Focused journal tests | PASS: 10 |
| Focused Unix PTY | PASS: 10 (9 lifecycle + 1 parser) |
| Outer interactive PTY, full suite | PASS: 10; baseline resize alone FAIL before the fix |
| Render fallback 16x8 and recovery | PASS |
| Default workspace | PASS: 464 library, 4 analytics CLI, 19 sync CLI, 5 TUI CLI, 10 PTY |
| All-features workspace | PASS: 462 library, 2 analytics CLI, 19 sync CLI, 5 TUI CLI, 10 PTY |
| Python / Ruff lint and format / contracts | PASS: 666 tests, including 28 probe tests; 6 schemas, 16 positive and 5 negative fixtures |
| PowerShell 7.5.4 on cloud Linux | PASS: corrected runner returns UNVERIFIED/2 at native host guard; baseline automatic variable assignment reproduces VariableNotWritable |
| 0.164.0 isolated initialize only | FAIL: unsupported_runtime (expected fail-closed result), all metadata fields valid |
| Corrected physical Arch qualification | UNVERIFIED: designated device unavailable |
| Native Windows live Codex | UNVERIFIED: no authorized native environment or bounded live requests |

The existing Windows ACL/ownership, journal replacement/locking/CAS, Job Object,
approval and no-replay implementation is unchanged. Native CI and release-bundle
results are recorded separately once Actions completes; the local Linux results
above cannot establish those platform results.

## Native CI correction — Darwin descriptor revocation

The first fix iteration (7f11f67, CI 42751694853) exposed Darwin's slave-descriptor
revocation when a controlling session leader exits. All application interactions
completed, but tcgetattr on the retained slave failed after exit; crash-reset
checks likewise used revoked descriptors. The harness now reopens the same named
slave for post-exit mode inspection, explicit crash reset and restart stdio.
It never resets modes before a normal-exit assertion. A focused test verifies
that reopening preserves an intentionally raw mode, rather than concealing it.
Restoration assertions remain unchanged in strength. Native CI must verify this
correction on both macOS architectures; the earlier failed run is not PASS evidence.

### Darwin lifecycle follow-up

Native ARM64 run 42752130976 at 4044275 passed 8/11 PTY tests; three crash
restarts found a zero-sized terminal after session revocation. Crash reset now
restores the harness window to 120x40 as well as termios. Normal-exit verification
now uses a small /bin/sh supervisor as the session leader: the compiled TUI exits,
the supervisor records its status and waits for the harness, and the harness
asserts termios plus alternate-screen/cursor restoration **before** releasing the
session leader. This prevents Darwin's terminal revocation/reset from concealing
a mode-restoration failure. A bounded wait and group cleanup cover supervisor
failure. Only crash reset is permitted to change modes; normal-exit checks never
do so. Old failed runs remain failures, superseded queued runs are cancelled.

Native Intel CI at b4dde418 additionally exposed a race when mode inspection ran
again after releasing the supervisor. Verification/release is now performed once;
post-release polling only waits for exit. Destructor cleanup tolerates an already
exited child so a failed assertion cannot cause a second panic/abort. Explicit
crash-test termination continues to require successful group termination.
