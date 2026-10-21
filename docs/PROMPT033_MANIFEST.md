# P033 implementation manifest

## Scope and source

- Prompt: P033, Native Codex App Server Client, Session Manager & Project Association Foundation.
- Starting `origin/main`: `80e264752294b61947041d6438e27020b023a54c` (P032 merge / PR #27).
- Feature branch: `feat/p033-codex-app-server`.
- Product and executable remain Codex Meter / `codex-meter`; implementation is Rust. No release,
  tag, TUI, plugin package, prompt execution, or cloud integration is part of this change.
- Official references inspected: Codex App Server documentation and the upstream App Server and
  App Server protocol sources. The installed executable's generated stable JSON Schemas were
  inspected from `codex-cli 0.159.2`; they were generated under `/tmp` and are not committed.

## Implementation

- Added a reusable `codex_app_server` application module with a direct child process, default stdio
  JSONL transport, one-time initialization, bounded request IDs and messages, response matching,
  independent bounded notification handling, typed errors, deadlines covering writes and replies,
  stderr draining, process supervision, and child cleanup.
- Server-initiated requests receive a JSON-RPC error and are never auto-approved. Raw stderr,
  arbitrary RPC error text, notifications payloads, Codex home path, session previews, turns, and
  filesystem paths are not retained in public models or diagnostics.
- Added paginated session listing (`sessions`, `--all`, `--archived`, `--active`, `--limit`,
  `--json`), metadata-only `session show`, explicit `session resume`, and confirmed `session fork`
  with `--yes` opt-in. Resume only loads the thread; fork copies its conversation history. Neither
  starts a turn.
- Added live paginated `models` output and project grouping through the Codex project ID, read-only
  Git repository/worktree metadata, or an exact accessible non-Git working directory. Opaque
  project aliases exist only for one command response.
- Session origin is explicitly `local_codex`. Pin and loaded values remain unavailable if the
  stable protocol does not provide them. P031/P032 `usage_link` remains unavailable because no
  deterministic identity link was verified. Session commands do not write the SQLite evidence
  database.

## Runtime and safe local observation

The implementation environment has `/opt/codex/bin/codex` at `codex-cli 0.159.2`. Its
`codex app-server --help` verified the default stdio mode. Stable schema generation and source
inspection verified `initialize`, `thread/list`, `thread/read`, `thread/turns/list`,
`thread/loaded/list`, `thread/resume`, `thread/fork`, and `model/list`; no experimental API is
enabled. App Server documentation identifies the interface as experimental/unsupported for
production, so this PR makes no stable cross-version or production guarantee.

The three safe read-only development commands were run against that local runtime after
implementation: `sessions --json` succeeded and reported 0 sessions, `models --json` succeeded and
reported 11 catalog entries, and `projects --json` succeeded and reported 0 project groups. No
individual session was inspected, resumed, or forked. These observations describe this execution
environment only; they do not validate the user's reference Arch desktop or imply account-wide
model access.

## Validation

The P033 Rust suite contains 34 fake-server and project/session tests. It covers the handshake and
initialization failures; errors and missing CLI; successful, concurrent, and out-of-order replies;
notifications and server-request rejection; malformed/oversized messages; response and blocked-write
timeouts; broken pipes; early exit and server shutdown; stderr flooding; cleanup; large writes;
session pagination/status/identity semantics; metadata-only read/resume/fork; unknown threads and
unsupported methods; empty and optional model catalog values; and project grouping, independent
repositories, worktrees, non-Git directories, inaccessible evidence, and path privacy.

| Gate | Result |
| --- | --- |
| `cargo fmt --manifest-path rust/Cargo.toml --all -- --check` | Passed |
| `cargo clippy --manifest-path rust/Cargo.toml --workspace --all-targets --all-features -- -D warnings` | Passed |
| `cargo build --manifest-path rust/Cargo.toml --workspace --locked` | Passed |
| `cargo test --manifest-path rust/Cargo.toml --workspace --all-targets --locked` | Passed: 358 library, 4 analytics CLI, 19 sync CLI tests |
| Python tests | Passed: 638 |
| Ruff lint and format | Passed |
| Python lock check | Passed |
| Contract validation | Passed: 6 schemas, 16 positive fixtures, 5 negative fixtures |
| P029 installed-runtime smoke | Passed on Linux x86_64 |
| P030 version policy | Passed |
| P030 release-artifact bundle and extracted smoke | Passed on `b6fbcc9333f6e0dfdbf05281878e709a1ee51953`; repeated Linux bundles matched at SHA-256 `cd71c82f2952109e80738ef4c2bf5f66501e61d498f988c3b1636f8af627cdef`, and the extracted installed-worker smoke passed |
| P031 sync / P032 reporting regression | Passed in the full Rust workspace suite |
| GitHub Linux/macOS/Windows CI | Passed on `b6fbcc9333f6e0dfdbf05281878e709a1ee51953` for push and PR events, including all installed-runtime and platform-bundle jobs |
| `git diff --check` | Passed on the reviewed staged diff |

An additional `cargo test --workspace --all-targets --all-features --locked` run reproduced two
pre-existing analytics source-mode failures. `--all-features` enables `release-artifact`, which
intentionally disables checkout worker discovery; those two tests specifically require source
mode. This is an unsupported test-feature combination, not a P033 or release-artifact contract
failure. The corrective validation is the repository's default workspace test command plus the
separate release-artifact installed-worker smoke. The normal tests were not weakened or skipped.

## Device gate, PR, and handoff

The reference Arch Linux desktop was not accessible. Its device gate remains pending under the
existing P032 project policy; synthetic fixtures and the separate 0.159.2 runtime are not
substitutes. Keep the PR Draft until the reference-device report is reviewed. Do not merge or
publish a release from this change.

The implementation is on [`feat/p033-codex-app-server`](https://github.com/nghianguyen150612/Codex-Meter/tree/feat/p033-codex-app-server), with feature commit `7d7da7aa93fd30c89f3c2f24830fac8bb734ff72` and EOF-classification fix `b6fbcc9333f6e0dfdbf05281878e709a1ee51953`. The initial PR run exposed a macOS timing race in process-exit classification; the fix polls the child directly after stdout EOF, and the corrected-head test and release workflows passed. Draft PR [#28](https://github.com/nghianguyen150612/Codex-Meter/pull/28) targets `main` and remains Draft under the device gate above.

P034 can build a TUI over the exported session/model/project services. It should preserve the
metadata and evidence separation, explicit confirmation for history cloning, and no-turn semantics.
Persistent user aliases, a prompt composer, quota-aware model recommendations, plugin packaging,
and Codex Cloud/ChatGPT Work support require separate scoped work and verified interfaces.
