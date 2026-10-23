# PROMPT038A qualification record

## Status

- Qualification status: **PARTIAL**.
- P038 remains **PARTIAL**. PR #33 remains draft and must not merge while G4 or G5 is unverified.
- Snapshot date: 2026-10-23.
- Repository: nghianguyen150612/Codex-Meter.
- Branch: feat/p038-windows-execution-reliability.
- PR: [#33](https://github.com/nghianguyen150612/Codex-Meter/pull/33).
- Qualification baseline source SHA: 310e6a83f2b3128477c05bbb28805f73444874d2.
- origin/main and PR base SHA at preflight: 352f4e0c499872f31205d75680172b8c1c8341a0.
- Starting working tree: clean. Local checkout was created from origin/pr/33 and left on the existing P038 branch.

## Preflight findings

The PR state was checked through GitHub and a clean local checkout:

- PR #33 was open, unmerged, draft, based on main, and headed by
  310e6a83f2b3128477c05bbb28805f73444874d2.
- The fetched origin/main SHA was 352f4e0c499872f31205d75680172b8c1c8341a0.
- The PR description and P038 manifest incorrectly said native Windows CI was pending and described
  run 39887178698 as still in progress. Those statements were corrected.
- No changes were made to the unrelated checkout found in the initial workspace.

## Environment classification

The available execution host was an isolated Debian GNU/Linux 13.6 environment, not an Arch device
or Windows machine:

| Field | Observed value |
| --- | --- |
| Distribution | Debian GNU/Linux 13.6 (trixie) |
| Host classification | Isolated container/managed execution environment |
| Kernel | Linux 6.18.55 |
| Architecture | x86_64 |
| Workspace filesystem | overlay, read-write, relatime; overlay options included index=off and metacopy=off |
| Rust toolchain | rustc, cargo, and rustup were unavailable on PATH |
| Rust target | Not available in this session |
| PTY | Standard input was not a TTY; Python openpty creation was available |
| Codex CLI display version | codex-cli 0.159.0-alpha.3 |
| App Server userAgent | Not probed here; this Linux CLI is not the Windows qualification target |
| Authentication | Not inspected |
| Native Windows capability | Unavailable |
| Arch physical reference device | Unavailable |

No user device was accessed remotely. No credentials or Codex login state were requested, copied, printed,
exported, or stored. No live task or external prompt was sent. Debian/container results are not used as
Arch reference evidence.

## CI and release evidence

Workflow-run metadata and job summaries were retrieved from GitHub before relying on them.

| Run | Workflow result | Head SHA | Job result |
| --- | --- | --- | --- |
| [39887178698](https://github.com/nghianguyen150612/Codex-Meter/actions/runs/39887178698) | **PASS** | 899447a9584efed259358a3c7a870083e640b338 | Earlier P038 implementation CI; all workflow jobs passed. The run head was not the current PR head. |
| [39888358799](https://github.com/nghianguyen150612/Codex-Meter/actions/runs/39888358799) | **PASS** | 310e6a83f2b3128477c05bbb28805f73444874d2 | 16 of 16 jobs succeeded, including Linux, native Windows, macOS Intel/ARM64, Rust quality, Python quality, contracts, and all installed-runtime cells. |
| [39888358815](https://github.com/nghianguyen150612/Codex-Meter/actions/runs/39888358815) | **PASS** | 310e6a83f2b3128477c05bbb28805f73444874d2 | 6 of 6 jobs succeeded, including shared wheel, Linux/Windows/macOS Intel/macOS ARM64 bundles, and aggregate validation. |

Run 39888358815 produced the shared wheel, four platform candidate bundles, and aggregate candidate
artifact. These are release-candidate validation results, not a public release.

The final CI job logs reported these platform test counts, with zero failures:

- Ubuntu: 463 library, 4 analytics CLI, 19 sync CLI, 5 TUI CLI, and 9 Unix PTY tests.
- macOS Intel and ARM64: 463 library, 4 analytics CLI, 18 sync CLI, 5 TUI CLI, and 9 Unix PTY tests
  on each architecture.
- Windows: 461 library, 4 analytics CLI, 17 sync CLI, 5 TUI CLI, and 4 synthetic ConPTY tests.

The existing local evidence record also reports that the all-features Rust workspace suite passed
at 899447a9584efed259358a3c7a870083e640b338: 461 library, 2 analytics CLI, 19 sync CLI, 5 TUI
CLI, and 9 Unix PTY tests. The later 310e6a83 commit changed documentation only. This session could
not rerun Rust commands because Cargo is unavailable in the execution host.

The native Windows CI result covers the journal backend and synthetic ConPTY fixture. It does not
prove live execution using the user's installed Codex App Server. Linux CI and macOS CI likewise do
not satisfy the Arch reference-device gate.

## Qualification results

| Gate | Result | Evidence |
| --- | --- | --- |
| G1 — Baseline | **PASS** | PR state, branch, starting SHA 310e6a83, base SHA 352f4e0, and clean checkout verified. |
| G2 — Cross-platform CI | **PASS at 310e6a83** | Final CI run 39888358799 passed all 16 jobs at the qualification baseline. The P038A docs/scripts change still requires the current PR head checks to be green before merge; no new functional claim is inferred from it. |
| G3 — Windows native security | **PASS** | Native Windows journal tests passed in run 39888358799, including current-user DACL/owner checks, additional ACE rejection, ancestor validation, reparse/hard-link rejection, local fixed-volume enforcement, inaccessible journal handling, atomic replacement, LockFileEx coordination, and revision CAS. |
| G4 — Windows live installed Codex | **UNVERIFIED / RELEASE BLOCKER** | No qualified native Windows environment, audited installed runtime handshake, authentication check, or live scenario evidence was available. The synthetic ConPTY tests do not satisfy G4. |
| G5 — Arch reference device | **UNVERIFIED / RELEASE BLOCKER** | No native physical Arch x86_64 reference device was available. The local Debian container is not a substitute. |
| G6 — No replay | **PASS** | Unix PTY and Windows synthetic ConPTY crash/restart tests preserve uncertainty, require exact thread/turn identities, and confirm no second turn/start is submitted automatically. |
| G7 — Evidence | **PASS after P038A commit and PR-body update** | P038A manifest, P038 manifest, platform matrix, recovery docs, test procedures, and PR description are synchronized. |
| G8 — Merge readiness | **BLOCKED** | Mandatory G4 and G5 are unverified; PR #33 must remain draft and unmerged. |

Power-loss durability is **UNVERIFIED** and is not an additional release prerequisite under this task.
Successful process-crash recovery, write ordering requests, atomic namespace replacement, OS crash
durability, and hardware power-loss behavior are separate claims. No hardware power-loss test was
performed.

## Arch reference procedure

The Arch runner is scripts/p038a-qualify-arch.sh. Use a fresh checkout of the current PR head on
the actual physical Arch Linux x86_64 reference device:

~~~sh
git clone https://github.com/nghianguyen150612/Codex-Meter.git Codex-Meter-p038a
cd Codex-Meter-p038a
git fetch origin pull/33/head:refs/remotes/origin/pr/33
git checkout --detach refs/remotes/origin/pr/33
bash scripts/p038a-qualify-arch.sh --confirm-reference-device
~~~

The script refuses to qualify a non-Arch or non-x86_64 host, an identified container or virtual
machine, a dirty tree, or a non-x86_64 Linux Rust host. It records distribution, kernel, architecture,
tested filesystem type and mount options, PTY availability, Rust version/toolchain/target, Codex CLI
display version, actual App Server initialize userAgent, commit SHA, exact commands, exit codes, and
test result counts. The App Server probe sends only initialize and initialized JSON-RPC messages; it
does not submit a task or read authentication state. The explicit confirmation flag attests that
this is the designated physical reference device; do not use it for another host.

The test commands include:

~~~sh
cargo fmt --manifest-path rust/Cargo.toml --all -- --check
cargo clippy --manifest-path rust/Cargo.toml --workspace --all-targets --all-features -- -D warnings
cargo test --manifest-path rust/Cargo.toml -p codex-meter --lib codex_app_server::execution_recovery
cargo test --manifest-path rust/Cargo.toml -p codex-meter --test pty_execution_recovery -- --test-threads=1
cargo test --manifest-path rust/Cargo.toml --workspace --all-targets
cargo test --manifest-path rust/Cargo.toml --workspace --all-targets --all-features
~~~

These repository tests exercise the Unix journal on the actual checkout filesystem, including
permissions, corruption, replacement, locks, CAS, deterministic persistence faults, approval,
cancellation, interruption, process termination, restart reconciliation, and no replay. The runner
prints only test-result summaries and failing test names, never raw PTY output or journal content.
If any host guard fails or the tests fail, record the gate as FAIL or UNVERIFIED with the sanitized
diagnostic; do not replace the device result with container evidence.

## Windows native and live-runtime procedure

The Windows runner is scripts/p038a-qualify-windows.ps1. Use a fresh checkout of the current PR
head on native Windows x64 PowerShell:

~~~powershell
git clone https://github.com/nghianguyen150612/Codex-Meter.git Codex-Meter-p038a
Set-Location Codex-Meter-p038a
git fetch origin pull/33/head:refs/remotes/origin/pr/33
git checkout --detach refs/remotes/origin/pr/33
.\scripts\p038a-qualify-windows.ps1
~~~

The runner records Windows build/architecture, local journal volume type and filesystem, administrator
role, Rust version and target, Codex CLI display version, and the App Server initialize userAgent.
It runs formatting, Clippy, focused journal tests, native Windows ConPTY tests, and all workspace
targets. Journal and temporary test data use a uniquely named directory under LocalAppData; the
runner restores its process environment and removes only that created directory. It does not change
the normal Codex configuration or login state. It does not print the current user's SID. Test output
is reduced to command names, exit codes, test counts, and failed test names.

The probe reads the actual App Server initialize response fields used by Codex Meter:
userAgent, platformFamily, and platformOs. It validates but discards codexHome. The audited identity
must be exactly codex_cli_rs/0.163.0. A CLI package version string alone cannot pass this check.
The runner does not inspect authentication. Verify the existing account session locally in Codex;
do not capture or share account details, auth files, tokens, or credentials. If the exact runtime is
missing, do not broaden the allowlist. The repository installation guides install Codex Meter, not
Codex CLI, and do not document installation of the audited Codex 0.163.0 runtime. Leave G4
unverified unless the user has an approved local installation procedure; do not modify existing
Codex configuration or login state.

The PowerShell runner covers native security and synthetic ConPTY behavior, then runs both the
default and all-features workspace suites. It exits with code 2 after passing those native tests
because the mandatory live scenarios are still unverified. G4 additionally requires
a separate, explicitly authorized, bounded live task exercise through Codex Meter. The runner sends
no prompt. Before any live request, the user must explicitly authorize the number and scope of task
submissions because they can consume account quota or contact an external service. Use a disposable
workspace and non-sensitive fixture. Do not use the user's normal repositories or destructive
commands.

For the live exercise, first verify the existing account session locally and record only whether it
is available. Start Codex Meter from a disposable workspace with the already installed audited
runtime. Use a read-only sandbox for the normal lifecycle, interruption, and restart checks where
possible. Approval tests may use one file inside the disposable workspace and must keep the
default-deny policy. Parent-termination and restart checks require intentionally closing or
terminating the Codex Meter process while a bounded request is active; after restart, reconcile only
the exact known thread and turn. If either identity is missing or the server cannot establish the
outcome, leave it unresolved and do not submit again. Do not hand-edit the normal journal to create
a revision conflict; the native Windows journal tests already cover stale-revision rejection.
Record each scenario separately, and stop when the authorized request count or scope is reached.

Record the following live scenarios individually, with PASS/FAIL/UNVERIFIED and sanitized evidence:

| Scenario | Acceptance evidence |
| --- | --- |
| App Server startup and JSON-RPC initialization | Initialize probe succeeds and emits initialized; only safe userAgent/platform metadata is recorded. |
| Runtime compatibility | App Server userAgent is exactly codex_cli_rs/0.163.0; a different value fails closed. |
| Normal execution lifecycle | One explicitly authorized bounded request reaches a terminal Codex status. |
| Approval and cancellation | A disposable fixture request is shown with default-deny behavior; allow-once and cancellation behave as selected. |
| User interruption | Explicit interrupt acknowledgement remains nonterminal until authoritative interrupted status arrives. |
| Process-tree cleanup | Closing Codex Meter terminates its App Server process tree through Job Object cleanup. |
| Parent termination and restart | Forced parent termination leaves recovery visible after restart and the App Server descendant exits. |
| Revision conflict | A stale journal revision is rejected; do not edit the normal journal to manufacture this case. |
| Uncertain outcome reconciliation | Exact known thread and turn are read; missing identity or evidence remains unresolved. |
| No replay | Restart/reconciliation does not submit another turn, resume, approval, or interrupt automatically. |

Some scenarios require live turns and may consume quota. A user must authorize and bound those requests
before running them. Native synthetic tests remain separately labeled and cannot substitute for
live-runtime results.

## Sanitized evidence format

Return only a sanitized record with this shape:

~~~text
commit=<full SHA>
host_class=<physical Arch x86_64 | native Windows x64>
os=<distribution or Windows build>
kernel_or_arch=<kernel and architecture>
filesystem=<filesystem type>
mount_options=<options only; omit device source and mount path>
rust=<version>
rust_target=<target triple>
codex_cli_version=<display version>
app_server_user_agent=<userAgent or unavailable>
native_tests=<PASS/FAIL/UNVERIFIED>
live_scenarios=<scenario names and PASS/FAIL/UNVERIFIED>
commands=<relative command and exit code>
test_counts=<passed/failed/ignored counts>
diagnostics=<sanitized failure summary only>
~~~

Do not include hostnames, absolute user paths, volume identifiers, SIDs, prompts, terminal output,
source text, thread/turn IDs, email addresses, account details, credentials, or raw journal contents.

## Merge decision

P038 is **not merge-ready** while G4 or G5 remains unverified. Keep PR #33 draft and do not merge.
After the Arch procedure and Windows live scenarios pass, recheck required PR checks at the final
head, update this record and the PR body, and only then make the merge decision under the repository's
normal review and branch-protection rules.
