# P045A: opt-in live account quota reader

## Scope and status

P045A adds one explicit, single-shot, read-only Codex App Server quota request.
It reuses `CodexAppServer::connect`, its initialized stdio transport, bounded
message handling, request ID checks, timeout, process cleanup, and sanitized
typed errors. It does not add a second RPC client, automatic polling, database
storage, login, task execution, or reset-credit use.

Starting base: P044 verified HEAD
`084c097c5b3cbe88ac718cb3683afe1e066bb7ec`, branch
`feat/p044-task-coverage-quota-evidence`. Delivery target is one stacked Draft
PR from `feat/p045-live-quota-readonly` to that P044 branch. Current PR/CI
status is recorded after pushing the implementation.

## Official contract audit

- Official reference: [Codex App Server](https://learn.chatgpt.com/docs/app-server),
  which names `account/rateLimits/read` as the ChatGPT rate-limit read and
  documents the single-bucket and keyed multi-bucket payloads.
- Audited Codex CLI version: `0.163.0`.
- Audited upstream commit:
  [`83086188ae0614bd882818ff1aa23f3194962012`](https://github.com/openai/codex/tree/83086188ae0614bd882818ff1aa23f3194962012/codex-rs/app-server-protocol).
- Pinned evidence: `codex-rs/app-server-protocol/src/protocol/common.rs`
  (`GetAccountRateLimits` method and optional params); v2 account protocol
  definitions; generated `schema/json/ClientRequest.json`;
  `schema/json/v2/GetAccountRateLimitsResponse.json`; and upstream App Server
  rate-limit tests.
- Method and params: `account/rateLimits/read` with
  `supportsLunaReserve: false` and `excludeResetCreditDetails: true`. Both
  fields are in the pinned optional params schema. The request does not enable
  Luna Reserve experiment participation and asks the server to skip reset
  credit detail lookup.
- Response evidence: legacy `rateLimits` snapshot and optional
  `rateLimitsByLimitId` map; bucket IDs/names; primary/secondary windows;
  integer `usedPercent`; optional `windowDurationMins` and Unix-second
  `resetsAt`; optional plan and account metadata. Account IDs, names, email,
  plan metadata, aliases, reset credits, upsell payloads, and unknown provider
  fields are discarded.
- Authentication: the official account-service note lists ChatGPT, externally
  managed ChatGPT tokens, agent identity, and personal access tokens; it says
  API-key-only and Bedrock auth do not work for ChatGPT account-service reads.
  The live reader relies on Codex's existing auth state, never reads secrets,
  and never starts login. The documentation does not promise identical
  behavior for every quota reader runtime.
- Compatibility limit: schema audit is static evidence only. No runtime is
  inferred compatible by version number. Unsupported methods surface the
  existing `unsupported_method` error. The P036/P038 execution allowlist is
  unchanged.

## Adapter and normalization

`CodexAppServer::read_rate_limits()` issues one typed method call on the
initialized client and returns a normalized `AccountRateLimitsSnapshot`. It
never returns or stores the unfiltered RPC `Value`.

When the multi-bucket map exists, it is authoritative. The `codex` bucket is
the main Codex meter; its embedded `limitId`, when present, must agree with the
key. The legacy view is used only when the map is absent. When both views
exist, inconsistent normalized windows or identity are reported as an
ambiguous result. Other Codex-related buckets, other buckets, and unknown
legacy identity are differentiated by fixed enum-like labels; arbitrary
provider bucket labels are not emitted.

Primary/secondary positions do not determine meter type. Each window is
classified independently from duration using P010's unchanged inclusive
ranges: 285–315 minutes for five-hour and 9,576–10,584 minutes for weekly.
Short windows (including 15 minutes), missing duration, duplicate supported
meters, and missing main identity are not guessed. `usedPercent` follows the
pinned integer field contract and must be finite and within 0–100. Observed
zero and 100 remain observed; remaining is derived. Optional reset seconds are
validated and converted to UTC. Missing percentage/duration/reset values stay
unavailable. The acquisition timestamp is generated after the response is
received, not from task timestamps.

## CLI and evidence boundary

`codex-meter quota --live [--json]` is the sole live entry point. `--live` is
incompatible with `--history`, `--after`, `--limit`, and `--database`. Without
it, the current SQLite reporting path remains in use. The live route never
opens SQLite, persists snapshots, or polls.

JSON schema version `1.0.0` includes the source, acquisition timestamp,
overall and per-meter availability, observed and derived percentage
provenance, validated reset timestamp, typed warnings, fixed bucket identity,
and account-level provenance. The human result explicitly says task
attribution and forecast qualification are not established.

**An App Server quota snapshot is account-level evidence and does not
authorize P044 task-specific reconciliation.** The adapter does not create a
P044B1 `AssociationCredential`, `QualifiedQuotaEvent`, task observation, or
estimator input. It provides no task-specific consumption, before/after task
association, complete token coverage, exclusive attribution, or official
token allowance.

## Privacy and read-only guarantees

The result cannot include email, account ID, access token, raw JSON-RPC
response, arbitrary provider fields, prompt, transcript, or process path.
There is no Codex Meter database mutation. No login, logout, turn start/steer/
interrupt, or rate-limit reset-credit consumption RPC is sent. Transport
stderr remains drained and discarded.

## Validation

Offline fake App Server tests assert the exact RPC and params, happy and legacy
paths, duration-based/reversed slot classification, zero/100 values, missing
fields, short/unsupported windows, duplicate and conflicting views, unknown
and non-main bucket identity, percentage/reset rejection, malformed and
oversized responses, incorrect IDs, authentication failure, unsupported
method, and redaction. Existing client/transport tests cover process startup
failure, timeout, shutdown, framing, server-request rejection, request ID
verification, and bounded responses. No real account is used.

Full local Rust, Python, contract, installed-worker, and Linux PTY results,
then exact-head CI and release workflow results, are recorded in the final
PR closeout. CI does not validate a physical device or authenticated live
quota accuracy. Installed-source compatibility, authenticated live-response
qualification, and Windows/Arch physical-device checks remain **UNVERIFIED**.

## Follow-on

P045B may add a separately versioned, explicitly opted-in account-snapshot
persistence and timestamped history contract after P045A is reviewed. Such
snapshots must remain account-level and must never be promoted automatically
to P044 task-specific evidence.
