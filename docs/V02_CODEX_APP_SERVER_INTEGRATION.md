# Codex App Server integration (P033)

P033 adds an unreleased, local-only session and model interface to the Rust CLI. P036 adds a
separate opt-in interactive execution capability in the Composer. Both use the installed Codex App
Server so Codex remains responsible for authentication, agent runtime, sandboxing, approvals, and
provider communication. The P033 management interface remains read-only with respect to turn
execution; P036 requires fresh preflight and explicit user confirmation before starting a task.

## Commands

Run these from a development checkout; they are not in the published v0.1.0 assets:

```console
cargo run --manifest-path rust/Cargo.toml -- sessions
cargo run --manifest-path rust/Cargo.toml -- sessions --all --limit 20 --json
cargo run --manifest-path rust/Cargo.toml -- sessions --archived
cargo run --manifest-path rust/Cargo.toml -- sessions --active
cargo run --manifest-path rust/Cargo.toml -- session show <THREAD_ID>
cargo run --manifest-path rust/Cargo.toml -- session show <THREAD_ID> --json
cargo run --manifest-path rust/Cargo.toml -- session resume <THREAD_ID>
cargo run --manifest-path rust/Cargo.toml -- session fork <THREAD_ID>
cargo run --manifest-path rust/Cargo.toml -- session fork <THREAD_ID> --yes
cargo run --manifest-path rust/Cargo.toml -- models
cargo run --manifest-path rust/Cargo.toml -- models --json
cargo run --manifest-path rust/Cargo.toml -- projects
```

`--all` and `--archived` select history scope; the default is non-archived. `--active` filters by
runtime status. `--limit` limits the returned sessions. JSON read commands include a schema version
and source label. Run a command with `--help` for its supported options.

## Process and protocol

The client starts `codex app-server` directly, without a shell, and uses its default stdio transport.
Messages are newline-delimited JSON. Each connection sends one `initialize` request using the honest
`codex-meter` client identity, waits for the response, and then sends `initialized`. The client does
not set `experimentalApi` and does not open a TCP or WebSocket listener.

The typed adapters use these stable methods:

| Method | Use |
| --- | --- |
| `initialize`, `initialized` | Connection handshake |
| `thread/list` | Paginated local history for active, archived, or all scopes |
| `thread/read` | One thread's metadata with `includeTurns: false` |
| `thread/loaded/list` | Loaded-state metadata, when supported |
| `thread/turns/list` | Optional paginated turn metadata with `itemsView: "notLoaded"` |
| `thread/resume` | Explicitly load a thread with `excludeTurns: true` |
| `thread/fork` | Explicitly clone a thread with `excludeTurns: true` |
| `model/list` | Current model catalog, including hidden entries |
| `account/rateLimits/read` | One explicitly requested, normalized account-level quota snapshot |

P033's compatibility was checked against `codex-cli 0.159.2` installed in that milestone's
implementation environment. P036 separately audited `codex-cli 0.163.0` and its generated stable
schemas before adding an isolated interactive capability; see the
[P036 task execution guide](V02_TASK_EXECUTION.md) for its allowlist and current limitations.
The installed executable's stable JSON Schemas were generated and inspected for initialization,
thread list/read/resume/fork/turns/loaded methods, and model list. The client sends all stable
`sourceKinds` in `thread/list`, uses `useStateDbOnly: true`, and follows each server cursor. Response
objects and bounded metadata fields are validated; duplicate cursors, malformed values, oversized
messages, and unsupported methods return typed errors. Notifications are queued by method name only.
Server-initiated requests receive an error response and are never approved automatically.

The integration uses Codex's existing authentication behavior. Codex Meter never reads
`~/.codex/auth.json`, extracts tokens, or stores credentials. App Server stderr is drained but not
retained or printed. The process is supervised and reaped on disconnect. The Codex App Server
documentation describes this interface as experimental and unsupported for production; P033 treats
it as an opt-in development integration and does not claim a stable cross-version contract.

## P045A live quota read

`codex-meter quota --live` starts the same initialized, bounded stdio client and
sends exactly one `account/rateLimits/read` request. It passes
`supportsLunaReserve: false` and `excludeResetCreditDetails: true`, which
prevents opting into the reserve experiment and avoids requesting reset-credit
details. The client does not poll `account/rateLimits/updated`, consume reset
credits, log in, or start/steer/interrupt a turn. Without `--live`, `quota`
continues to read local SQLite evidence. `quota --live` without `--save` retains
P045A's in-memory-only behavior and does not open or write SQLite.

The method was audited against Codex CLI `0.163.0`, upstream commit
`83086188ae0614bd882818ff1aa23f3194962012`, including the pinned generated
`ClientRequest.json`, `GetAccountRateLimitsResponse.json`, v2 protocol types,
and official [App Server reference](https://learn.chatgpt.com/docs/app-server).
The read method accepts optional params; its response has the legacy
`rateLimits` view and optional multi-bucket `rateLimitsByLimitId` map. The
schema's window percentage is integer `usedPercent`; duration and reset seconds
are optional. Runtime versions are not assumed compatible from the static
schema audit: an unsupported RPC is reported explicitly. This audit does not
change the separate P036/P038 task-execution allowlist.

The multi-bucket map wins when present, and `codex` identifies the main bucket.
If an embedded `limitId` contradicts its key or the legacy and multi-bucket
views disagree, the result is ambiguous. A legacy-only bucket requires a
matching `limitId`; missing identity is unavailable. Arbitrary bucket names
and `limitName` are discarded. Each window is classified by duration using
P010's existing five-hour and weekly ranges, so short windows cannot be
misreported as five-hour quota. Invalid percentages or reset timestamps reject
the response; missing metadata remains unavailable.

The response is projected into a versioned privacy-safe account snapshot with
observed percentages, derived remaining percentages, validated UTC reset times,
typed warnings, and a client-side acquisition timestamp. It contains no
account identifier, email, token, raw RPC JSON, prompts, transcripts, provider
labels, or process paths. Codex's existing auth is used; no credentials are
read or persisted. Unsupported method, auth, startup, and transport errors are
sanitized. **An App Server quota snapshot is account-level evidence and does
not authorize P044 task-specific reconciliation.**

The official account-service authentication note lists ChatGPT, externally
managed ChatGPT tokens, agent identity, and personal access tokens, and says
API-key-only and Bedrock authentication do not work for those account-service
reads. That note is not a compatibility promise for every `rateLimits/read`
runtime; unsupported or unavailable auth is returned as a typed error.

## P045B explicit persistence and history

`codex-meter quota --live --save` explicitly opts into one local write after a
successful typed `account/rateLimits/read`. Migration 7 stores a bounded,
checksummed account snapshot in `live_account_quota_snapshots`; it does not
alter P031 rollout evidence, P016 Observations, or P044 task reconciliation.
`--save` is required even with `--json`. Storage failure returns a sanitized
error and never reports a successful save. A repeat request performs a new
live read and receives a new opaque event ID, even when its quota values match
an earlier acquisition.

`codex-meter quota --history --source live` is read-only, bounded, and queries
only the live account snapshot table. Rollout history remains the default for
`quota --history` without a source selector. Pages use keyset ordering by exact
acquisition nanoseconds and snapshot ID, both descending; a cursor must refer
to a saved live snapshot. A version-6 database can still be queried read-only;
live history is empty there until a later explicit writer applies migration 7.

Persistence retains only normalized percentages, reset timestamps, typed
availability and warning codes, fixed bucket labels, the UTC acquisition time,
and fixed account-level provenance. It stores no account identifiers, email,
credentials, raw provider JSON, arbitrary provider labels, prompts, transcripts,
or filesystem paths. Snapshot movement remains account-level evidence and is
not joined to tasks or promoted to capacity-estimator evidence. The RPC and
SQLite transaction are not a distributed transaction; exactly-once remote
acquisition across crashes is not promised. See
[`PROMPT045B_MANIFEST.md`](PROMPT045B_MANIFEST.md).

## P046A live source diagnostic

`quota --live --diagnose [--json]` uses the same connection and performs one
`account/rateLimits/read`. It consumes the P045A normalized snapshot and
provides per-meter structural findings with fixed account-level provenance.
It does not write SQLite, save a live snapshot, poll, or start a model turn.
Task association, reset stability, external-use isolation, token coverage,
production accuracy, and estimator qualification remain unverified. Runtime
version metadata is bounded to a strictly parsed App Server user-agent version;
a match to the pinned audit version is not itself a compatibility claim.
Unsupported methods and other failures remain sanitized typed errors. See the
[P046A manifest](PROMPT046A_MANIFEST.md) for the versioned JSON contract and
qualification limits.

## Session fields and lifecycle

Session rows retain thread ID, session-tree ID when returned, name, creation/update timestamps,
runtime status, source, archive state, optional pin/loaded state, model/provider/reasoning metadata,
project ID, and fork relationships. A thread ID identifies one thread; a session ID can identify
its wider thread tree. These identifiers are not interchangeable.

`active`, `idle`, `notLoaded`, and `systemError` are runtime states, not completion states. Unknown
future statuses are preserved as unknown. The loaded flag is separate and may be unavailable when
the server does not implement `thread/loaded/list`. Archive state comes from the selected list
scope; `thread/read` does not establish that value. The stable schema inspected for 0.159.2 has no
pin field, so pin state is unavailable unless a server returns a supported `isPinned` value.
Unavailable timestamps, source, model, or project values remain unavailable rather than being
inferred.

`session show` reads metadata without resuming the thread. It does not load or persist transcript
content. Optional turn listing requests metadata-only items and rejects any response containing
turn items. Session previews, prompt bodies, messages, reasoning, tool input/output, and working
directory paths never enter the public Rust models or Codex Meter database.

`session resume` calls `thread/resume` only after the explicit command. It loads thread state inside
Codex App Server; it does not send a prompt, start a turn, or open an interactive terminal. The
message says what happened and what Codex Meter does not provide. `session fork` copies the source
conversation history into a distinct thread, preserves its returned parent relationship, and does
not start a turn or modify the parent. An interactive confirmation is required unless `--yes` is
provided. Unsupported methods fail with a capability error; no filesystem fallback is used.

## Model discovery

`models` reads the live `model/list` response and follows server cursors. It reports returned model
IDs, display names, default/hidden flags, reasoning options, default reasoning effort, and supported
input modalities when available. It does not hardcode model names or infer account access from a
name. A successful empty response is shown as an empty live catalog; an unsupported method or
unavailable App Server is an error, not an empty catalog. Model results are not cached.

## Project association and usage links

`projects` groups local sessions by the App Server's opaque Codex project ID when available. Where
that value is missing, it uses read-only Git metadata: common Git directories group linked worktrees
under one repository, while independent repositories remain separate. An accessible non-Git
working directory is its own group. Missing or inaccessible evidence remains unavailable. Aliases
such as `project-001` are in-memory labels for one command response; P033 does not save aliases,
paths, remotes, source files, or Git configuration.

The session detail may report an opaque Codex project ID or an association kind, but never emits the
working directory or Git remote. Git paths stay in memory only for grouping. Codex Meter does not
guess project identity from directory names.

P031/P032 remain authoritative for token and quota evidence. P033 reports
`usage_link: unavailable`: the App Server thread/session IDs have not been proven to match the
existing deterministic rollout identity. There is no timestamp, title, path, or substring join and
no SQLite write from these commands. Local token usage remains distinct from account quota movement.

## Local and cloud scope

The only supported origin is `local_codex`. Codex Cloud tasks and ChatGPT Work conversations are
not inferred from local thread metadata. Unsupported origins remain unavailable and require a
separate documented integration.

The session, model, and project services are reusable Rust application APIs. The P034 TUI calls
these services through one bounded background App Server worker rather than implementing another
connection. P033 itself did not add a full-screen TUI. Prompt composition, model switching, plugin
packaging, cloud integration, and persistent project tagging remain later milestones.

## Validation status

Synthetic process tests exercise protocol framing, pagination, redaction, errors, lifecycle,
timeouts, and cleanup. They do not establish compatibility with a user's Codex installation. The
reference Arch Linux device was not available during P033; its live-device gate remains pending.
See [the P033 manifest](PROMPT033_MANIFEST.md) for the exact runtime observations, gates, PR state,
and P034 handoff.

## References

- [Codex App Server documentation](https://developers.openai.com/codex/app-server/)
- [Codex App Server source](https://github.com/openai/codex/tree/main/codex-rs/app-server)
- [Codex App Server protocol source](https://github.com/openai/codex/tree/main/codex-rs/app-server-protocol)
