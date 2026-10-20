# PROMPT031 implementation manifest

## Status and Git workflow

- Status: **PARTIAL** until the Arch device gate, required GitHub checks, and PR closeout have been completed.
- Verified starting `origin/main` SHA: `bcabc4c20b0e4f877c2e01e936ddd79a1b10799e0`.
- Feature branch: `feat/p031-real-codex-ingestion`.
- Intended commit: `feat: ingest real Codex telemetry into durable SQLite history`.
- PR title: `Integrate real Codex telemetry ingestion and persistent usage history`.
- PR URL and CI results: pending.
- Merge: intentionally pending while the required live-device gate is unavailable.
- v0.1.0 tag, published assets, installer behavior, estimator mathematics, Python analytics schemas, and method versions were not changed.

## Implementation

- Native CLI command: `codex-meter sync`; also supports `--json`, `--codex-home PATH`, `--database PATH`, and `--help`. Existing `estimate` parsing and subprocess routing remain in place.
- Discovery precedence: explicit home, valid `CODEX_HOME`, then platform default. Scans active dated rollout directories and the source-audited flat archive root, skips symlinks, and reports compressed `.jsonl.zst` evidence as unsupported.
- Validated compatibility scope: Codex CLI `0.157.1` rollout fixtures and the repository's prior source audit at upstream revision `8f7a0f7a878199c6886600370e5be6bd37ca38a3`. No newer Codex version or live installation has been verified by this implementation environment.
- Canonical response use comes only from `token_usage_record.payload.usage`. Turn/thread cumulative values and token-count total/latest values are persisted with snapshot semantics. Missing fields remain NULL. Raw totals come from the source `total_tokens` field.
- Durable storage: migration 5, `0005_usage_evidence.sql`, adds typed token, quota, diagnostic, and source-boundary tables. Existing migrations are unchanged. Source checkpoint, typed evidence, boundary fingerprint, and applicable Observation updates share a transaction.
- Task completion produces a task-scoped Observation when source task identity is available. Its token counters are assembled from per-response records; P031 supplies no task quota delta, leaving five-hour and weekly evidence unavailable/incomplete.
- Default database: Linux `$XDG_DATA_HOME/codex-meter/codex-meter.sqlite3` or `~/.local/share/codex-meter/codex-meter.sqlite3`; macOS `~/Library/Application Support/codex-meter/codex-meter.sqlite3`; Windows `%LOCALAPPDATA%\codex-meter\codex-meter.sqlite3`.
- Live values retained: observed five-hour and weekly used percentages, source sample time, reset time when supplied, and plan metadata when supplied. No provider authentication, API calls, quota scraping, or Codex file writes occur.

## Privacy, persistence, and automated tests

The evidence ledger contains typed normalized values and hashed source/event identities, never arbitrary rollout JSON. Integration checks inspect the database and CLI output for fixture prompt, response, provider response ID, rate-limit payload, and path markers. Diagnostics preserve typed categories and hashes without the original malformed line. Default Linux storage is created with private directory/file modes.

The native CLI integration test suite covers explicit/environment/default source selection; missing/unsupported homes; active and archived rollout handling; copied-source ambiguity; compressed-source warnings; symlink skips; Unicode and space paths; incomplete-tail recovery; replay and append behavior; source replacement detection; trigger-induced transaction rollback and retry; per-response versus snapshot semantics; quota windows; NULL versus zero; task Observation completeness; and database/output privacy. Existing unit suites cover source decoding, normalization, quota classification, checkpoint integrity, and Observation contracts.

Local feature-branch gates passed in the implementation environment:

- `uv sync --project python --frozen` and `uv lock --check --project python`.
- Rust format check, Clippy with `-D warnings`, and workspace tests: 300 unit tests, 4 analytics CLI integration tests, and 19 sync CLI integration tests passed.
- Locked workspace build, Python suite (637 passed), Ruff lint/format, and contract validation (6 schemas, 16 positive fixtures, 5 negative fixtures).
- P029 installed-runtime smoke and P030 release-version policy check.
- P030 candidate installed-runtime smoke and PR CI remain pending until after the implementation commit and Draft PR are created.

These are fixture and local-runtime results only. No test output is claimed as real-device evidence.

## Arch Linux live-device gate

The reference desktop is not mounted in this execution environment. No actual Codex home, installed Codex version, live rollout, or personal token count was read. Live ingestion, duplicate rescan, post-restart recovery, and a subsequent normal usage event remain **pending**. Do not trigger a paid request specifically for this gate.

On the reference Arch device, from the feature checkout after building the development binary:

```bash
codex --version
if [ -n "${CODEX_HOME:-}" ] && [ -d "$CODEX_HOME" ]; then
  printf 'codex_home=found\n'
elif [ -d "$HOME/.codex" ]; then
  printf 'codex_home=found\n'
else
  printf 'codex_home=not_found\n'
fi

report_dir="$(mktemp -d)"
chmod 700 "$report_dir"
trap 'rm -rf "$report_dir"' EXIT
cargo run --manifest-path rust/Cargo.toml -- sync --json > "$report_dir/first.json"
cargo run --manifest-path rust/Cargo.toml -- sync --json > "$report_dir/replay.json"
jq -c '{status,supported_codex_cli_version,codex_cli_detected,sessions_seen,rollout_sources_seen,rollout_sources_processed,responses_added,saved_response_events,saved_raw_total_known_sum,saved_raw_total_known_events,saved_raw_total_missing_events,unsupported_compressed_sources,skipped_records,warnings}' "$report_dir/first.json"
jq -c '{responses_added,saved_response_events,saved_raw_total_known_sum,saved_raw_total_known_events,saved_raw_total_missing_events}' "$report_dir/replay.json"
diff -u \
  <(jq -S '{saved_response_events,saved_raw_total_known_sum,saved_raw_total_known_events,saved_raw_total_missing_events}' "$report_dir/first.json") \
  <(jq -S '{saved_response_events,saved_raw_total_known_sum,saved_raw_total_known_events,saved_raw_total_missing_events}' "$report_dir/replay.json")
```

The two separate invocations exercise process restart and exact replay. Then
continue normal user-authorized Codex work (do not send a request for testing)
and run one more sync:

```bash
cargo run --manifest-path rust/Cargo.toml -- sync --json > "$report_dir/after-normal.json"
jq -c '{responses_added,saved_response_events,saved_raw_total_known_sum,saved_raw_total_known_events,saved_raw_total_missing_events,warnings}' "$report_dir/after-normal.json"
```

Confirm that a newly saved per-response event corresponds to the normal
activity.
Inspect the local evidence with count-only queries and do not print payloads or
paths into the report:

```bash
case "${XDG_DATA_HOME:-}" in
  /*) data_home="$XDG_DATA_HOME" ;;
  *) data_home="$HOME/.local/share" ;;
esac
db="$data_home/codex-meter/codex-meter.sqlite3"
sqlite3 "$db" "SELECT record_semantic, COUNT(*) FROM token_usage_evidence GROUP BY record_semantic ORDER BY record_semantic;"
sqlite3 "$db" "SELECT meter_type, COUNT(*) FROM quota_snapshot_evidence GROUP BY meter_type ORDER BY meter_type;"
sqlite3 "$db" "SELECT COUNT(*) FROM runtime_checkpoints;"
sqlite3 "$db" "PRAGMA table_info(token_usage_evidence);"
test "$(sqlite3 "$db" "SELECT COUNT(*) FROM pragma_table_info('token_usage_evidence') WHERE name IN ('prompt','response','reasoning_text','raw_json','payload');")" = 0
printf 'forbidden_payload_columns=0\n'
```

The report should include only the CLI version, status/warning enums, event and
snapshot counts, whether the exact replay totals matched, whether a later
normal event added one event, migration version, and whether private-schema
checks were clean. Do not attach rollout files, database contents, prompts,
responses, or absolute home/database paths. If compatibility is unsupported or
the summary shows a coverage warning, retain it as a blocker rather than
claiming live support.

## Quality gates and P032 handoff

Required Rust/Python/contracts/release-runtime gates, diff review, PR CI, and
final main synchronization are pending until implementation is committed and
published as a Draft PR. The PR must remain Draft if live-device validation is
still outstanding. It must not be merged or published as v0.2.0 while that gate
is pending.

P032 can build token history/inspection over `token_usage_evidence` and
`quota_snapshot_evidence`. Keep raw per-response usage distinct from cumulative
snapshots and account-meter snapshots. Task quota attribution requires later
valid baseline, same-window, stabilization, and reconciliation evidence. P031
does not start P032.
