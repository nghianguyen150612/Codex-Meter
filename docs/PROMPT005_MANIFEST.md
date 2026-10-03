# Prompt 005 Manifest

## Identification and baseline

- **Prompt:** P005 — Codex Telemetry Surface Discovery and Evidence Audit
- **Title:** Audit Codex telemetry surfaces
- **Starting SHA:** `9cafc892a1c94959033cad539eab23e3a12d22ab`
- **Branch:** `main`
- **Verified P004 baseline:** `HEAD` exactly matched the expected merged P004 baseline before research. The starting worktree was clean.
- **Final SHA:** Not embedded because a commit cannot contain its own SHA; the completion report records it.

The expected P004 baseline was verified without resetting or rewriting any
legitimate work. No commits after the expected baseline were present.

## Discovery environment

- Installed Codex CLI: `codex-cli 0.157.1`.
- Platform: Linux x86_64.
- Local Codex home: `CODEX_HOME` was set; the personal absolute path is not recorded.
- Upstream repository: official `openai/codex` source.
- Upstream revision: `8f7a0f7a878199c6886600370e5be6bd37ca38a3`, inspected October 3, 2026.
- Upstream checkout: temporary detached checkout outside `Codex-Meter`; it was not vendored.

## Sources used

- Installed CLI metadata: `command -v codex`, `codex --version`, and `codex --help`.
- Local Codex home directory/file metadata and sanitized structural inspection.
- Local SQLite table/column schemas without row values.
- Official upstream rollout, history, protocol, state, and test sources at the pinned revision.

## Files created

- `docs/TELEMETRY_DISCOVERY.md`
- `docs/PROMPT005_MANIFEST.md`

## Files modified

- None outside the two P005 documents above.

## Confirmed findings

- Current local Codex persists session rollouts as dated JSONL files below the configured Codex home.
- Upstream defines active `sessions` and archived `archived_sessions` rollout roots, with plain JSONL and current `.jsonl.zst` support.
- Persisted records use a timestamp/optional-ordinal envelope with a top-level discriminator and payload.
- `token_usage_record.payload.usage` is the per-response usage candidate.
- `turn_token_usage`, `thread_token_usage`, and `token_count.info.total_token_usage` are cumulative snapshots; `token_count.info.last_token_usage` is the latest appended usage.
- Token fields include input, cached input, cache-write input, output, reasoning output, and total tokens.
- Session metadata and turn/context records expose IDs, model/provider/version/reasoning/configuration evidence subject to availability.
- Task lifecycle markers include `task_started`, `task_complete`, and interruption/compaction evidence.
- SQLite state and thread-history databases provide derived metadata and cursor evidence but are not the primary raw-token source.

## Unresolved findings

- The mathematical identity of upstream `total_tokens` relative to component fields is unproven.
- It is unproven whether reasoning output is included in output totals for every provider.
- Safe derivation of `uncached_input` by subtraction is unproven.
- Session-end/resume markers, crash-tail frequency, watcher notification behavior, and exact archive timing need adapter tests.
- Service-tier-to-speed semantics and quota/plan semantics remain unresolved.
- Local CLI telemetry is not proven to represent complete account or Work/web usage.

## Privacy precautions

- No raw JSONL records were copied into the repository.
- No prompts, responses, reasoning text, tool payloads, source code, paths, CWD, Git data, account identifiers, credentials, cookies, API keys, OAuth tokens, or authorization headers were recorded.
- Local inspection emitted only names, sizes, timestamps, keys, discriminators, types, and numeric-field paths.
- The report recommends an explicit telemetry allowlist before future parsing.

## Validation and implementation boundary

- Repository contracts, ADRs, schemas, fixtures, roadmap, and P001–P004 manifests were inspected and preserved.
- No Rust runtime code or production parser was added.
- No runtime dependency was added.
- No real user telemetry fixture was added.
- No quota implementation, authentication, UI scraping, watcher, cursor, SQLite storage, or task tracker was added.
- `docs/ROADMAP.md` was left unchanged because discovery did not demonstrate a need to reorder P006–P009.
- `just` was unavailable, so its direct equivalents were run: Cargo fmt check, Clippy, Rust tests, Ruff lint, Ruff format check, Python tests, contract validation, and `git diff --check`; all passed.
- Final Git hygiene checks are recorded in the completion response.

## Final commit and unrelated-work handling

- Exactly one logical commit is required with message: `docs: audit Codex telemetry surfaces`.
- Only the two P005 documents are to be staged.
- The final commit SHA is reported after commit creation rather than embedded here.
- Push/PR results are reported only if genuinely available.
- No unrelated work was present at baseline or modified by P005.
