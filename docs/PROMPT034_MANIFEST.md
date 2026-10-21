# PROMPT034 implementation manifest

## Status and baseline

- Status: **PARTIAL** until the reference Arch Linux live-device gate is completed and reviewed.
- Verified starting `origin/main`: `ffa43c4a88e0d3c1c8f62f271e88c57cffa42b38`, the P033 merge
  commit from PR #28.
- Feature branch: `feat/p034-ratatui-dashboard`.
- The published v0.1.0 tag `171b54ee4f80c007ab6e0b4af3cd1bb22613e755` and its release assets
  are preserved. P034 does not change product, binary, schema, or Python method versions.
- Pull request and implementation commit identifiers are recorded after publication of the draft
  PR below.
- The Arch reference desktop is not attached to this environment. Its actual Codex version, real
  quota observations, and live terminal compatibility remain unverified.

## Implementation

P034 adds `codex-meter tui` using Ratatui 0.29.0 and Crossterm 0.28.1. The existing no-argument
command remains help. `tui --help` works without a terminal; an interactive TTY is required for
the dashboard, and redirected invocation fails before terminal escape sequences are written.
`--database PATH` consistently selects the existing local evidence database.

The five screens are Overview, Sessions, Projects, Models, and History. Ratatui rendering,
application state transitions, bounded background workers, and P031/P032/P033 services are
separated. SQLite reports use the existing read-only store; App Server access uses one initialized
connection on its worker. Result IDs discard obsolete responses. History uses P032 keyset pages of
50; terminal lists render at most 50 rows at once. The App Server service bounds its complete
metadata result. No benchmark claims are made for very large ledgers or session collections.

Overview shows canonical per-response token statistics and observed quota samples with exact
validated percentages, separately derived remaining values, timestamps, freshness, and coverage
warnings. Missing values remain unavailable; invalid percentages do not draw a bar; stale samples
remain visibly stale. Token snapshots and account quota evidence are not summed as task usage. The
screen labels local telemetry as incomplete account usage.

Sessions, project groups, and the model catalog reuse P033 services. Session token attribution
remains unavailable. Project aliases remain response-local. Model metadata is not an entitlement
claim. Loading a session requires an explicit `L` action and only resumes/loads the thread; it does
not submit a prompt or start a turn. `F` requires confirmation before cloning history. `s` requires
confirmation before P031 one-shot local sync writes the evidence ledger. Passive browsing does not
mutate the database. No `turn/start`, model switching, or transcript persistence is implemented.

The UI never parses raw rollout payloads. Read-only screens show sanitized P031/P032/P033 models,
stable error categories, and no credentials, prompt/response content, reasoning text, raw JSON,
filesystem paths, or Git remotes. `NO_COLOR` disables semantic colors; non-UTF-8 locales use ASCII
borders and sanitized non-ASCII labels. Arbitrary session titles are not rendered; path-like metadata
identifiers are redacted, and unsafe thread identifiers cannot be resumed or forked. Scoped
terminal cleanup restores raw mode, alternate screen,
and cursor visibility on normal exit, Ctrl+C, and unwinding.
Workers are signaled and joined after terminal restoration. Queued requests are abandoned; an
in-flight App Server request reaches its P033 deadline before its child is closed. An explicit sync
already in progress is allowed to finish its current ingestion operation.

## Validation

Local validation completed in the managed Linux container:

| Gate | Result |
| --- | --- |
| Rust formatting | Passed: `cargo fmt --manifest-path rust/Cargo.toml --all -- --check` |
| Clippy | Passed: locked workspace, all targets/features, warnings denied |
| Rust build | Passed: locked workspace build |
| Default locked Rust tests | Passed: 382 library, 4 analytics CLI, 19 sync CLI, 3 TUI CLI tests (408 total) |
| All-features locked Rust tests | Passed: 380 library, 2 analytics CLI, 19 sync CLI, 3 TUI CLI tests (404 total) |
| P031/P032 regression | Passed in the Rust workspace suite |
| P033 fake App Server regression | Passed in P033 and TUI worker tests; fake covers session list, project grouping, model catalog, resume, and fork |
| TUI rendering/state tests | Passed: responsive dimensions, low height, empty views, overlays, ASCII fallback, fractional/stale/invalid/missing quota, partial totals, long Unicode labels, stale results, confirmation handling |
| Redirected CLI behavior | Passed: no terminal control output; diagnostic identifies TTY requirement |
| Linux PTY lifecycle | Passed: launched against a missing local database, handled missing-evidence states, accepted `q`, verified ASCII output with `NO_COLOR` and a C locale, and restored cursor/alternate screen |
| Python tests | Passed: 638 |
| Ruff lint and format | Passed |
| Python dependency lock check | Passed |
| Schema/fixture validation | Passed: 6 schemas, 16 positive fixtures, 5 negative fixtures |
| P029 installed-runtime smoke | Passed: reproducible wheel, isolated install, relocation, source mode, determinism, concurrency, and read-only SQLite |
| P030 version policy | Passed: v0.1.0 policy unchanged |
| P030 extracted candidate smoke | Passed: local Linux bundle exercised after fresh extraction/relocation, installed worker, sufficient and insufficient estimates, deterministic JSON, version compatibility, and read-only SQLite |
| P030 aggregate candidate | Pending platform matrix; only the Linux bundle was built locally, and aggregate verification requires Linux, macOS x86_64/arm64, Windows, and wheel artifacts |
| `git diff --check` | Passed before final review |
| GitHub Linux/macOS/Windows CI | Pending Draft PR checks |

P032/P033 documented an `--all-features` test conflict: `release-artifact` intentionally omits
checkout worker discovery, while source-mode tests require it. P034 narrowly cfg-gates only those
source-worker unit and integration tests from release-artifact builds. They remain enabled and pass
in the default workspace test run. Release-artifact production worker behavior is unchanged; both
default and all-features workspace tests and all-features Clippy now pass.

The PTY check is a container smoke test, not Arch-device validation. The live reference gate remains
pending, and fixture/CI results do not establish real-world quota accuracy or compatibility. No
personal session was resumed or forked and no paid activity was generated.

## Device gate, PR, and handoff

The reference Arch Linux desktop was inaccessible in this environment. Under the project policy
recorded in P032/P033, keep this PR Draft and do not merge until a sanitized live-device validation
report is reviewed. Do not publish v0.2.0. PR state and source commit are recorded here after the
draft PR is created.

On the reference desktop, after reviewing `codex --version`, run `codex-meter tui` and validate
startup, overview evidence, quota states, session/project/model/history navigation, refresh, resize,
and clean terminal exit. Do not resume or fork a personal session. Do not create paid activity to
manufacture test evidence. Record only a sanitized version/status summary and whether quota displays
match persisted evidence; keep raw IDs, local paths, and the usage database local.

P035 may build on the read-only dashboard foundation. Prompt composition, model switching, quota
prediction, Codex Cloud/ChatGPT Work adapters, persistent project configuration, PR/CI integration,
plugin packaging, and cloud workspaces remain out of P034 scope and need separately verified
interfaces and authorization.
