# Codex Meter Technical Architecture

## Purpose and authority

This document defines the implementation boundaries for Codex Meter v1. It applies the product contract's **Track → Measure → Estimate → Compare** loop without changing its semantics. In particular:

```text
raw tokens != weighted/effective usage != plan quota
```

Codex Meter remains independent and unofficial, local-first, privacy-first, Plus-first, CLI-first, empirical rather than speculative, and confidence-aware. It does not claim an undocumented fixed OpenAI token allowance. The initial P003 contract and P004 repository bootstrap milestones are complete; versioned contracts and Rust/Python implementations are maintained in the repository.

## System architecture

```text
                    Local Codex / Work environment
                                 │
                     discovery-dependent sources
                                 ▼
                       ┌───────────────────┐
                       │  Source adapters  │
                       └─────────┬─────────┘
                                 ▼
                       ┌───────────────────┐
                       │ Rust tracker/agent│
                       └─────────┬─────────┘
                                 │
             ┌───────────────────┼───────────────────┐
             ▼                   ▼                   ▼
      Parser/ingestion     Quota adapter       Session state
             │                   │                   │
             └───────────────────┼───────────────────┘
                                 ▼
                   Provider-independent events
                                 ▼
                      Observation pipeline
                    (reconcile, detect resets)
                                 ▼
                  Rust-owned transactional storage
                                 ▼
                              SQLite
                                 │
                  read-only allowed analytics data
                                 ▼
                       Python analytics process
                                 │
                    versioned JSON result contract
                                 ▼
                 Rust validation/application services
                                 ▼
                    CLI capacity estimates
```

No exact telemetry location, file format, field, quota endpoint, or meter behavior is asserted here. Those mechanisms are unvalidated discovery inputs hidden behind adapters.

## Architectural layers

### Domain/core

Provider-independent concepts include task and session identity, token counters, quota measurements and windows, observations and their quality, estimator requests and results, reset events, timestamps, and configuration identity. Core types MUST NOT directly depend on Codex JSONL fields, filesystem layout, HTTP/RPC, SQLite rows, Python internals, or terminal rendering. Interfaces needed by application logic should be defined toward the core and implemented by infrastructure.

### Source adapters

Adapters isolate Codex session and local telemetry discovery, file/event watching, quota acquisition, Codex version discovery, and model/reasoning/speed discovery where evidence permits. They translate source-specific records into normalized provider-independent inputs before the observation pipeline. Codex-specific field names must not spread through the domain. Each exact source remains discovery-dependent and replaceable.

Platform adapters similarly isolate filesystem locations, watcher behavior, optional process discovery, configuration directories, path representation, installation/service management, and any future platform credential/keychain integration. Platform code cannot contaminate analytics or statistics.

### Parser and ingestion

Ingestion incrementally parses source records, maintains cursors/checkpoints, avoids duplicates, tolerates incomplete trailing records, classifies malformed records, and emits normalized token/session events. It must not persist prohibited source payload content. The Rust telemetry modules implement the parser and cursor boundaries.

### Observation pipeline

The provider-independent pipeline turns runtime evidence into candidate and finalized observations. It coordinates task/session start, progress, token-delta accumulation, task/session completion, before/after quota samples, meter stabilization, reset detection, reconciliation, per-domain quality, and finalization or invalidation. Source acquisition remains outside this logic wherever practical.

### Storage

SQLite is the durable local store for runtime checkpoints, observations, and migration/version metadata. Rust creates the database, owns migrations and primary writes, and performs transactional runtime updates. Analytics reads existing observations through P017's strict read-only Python reader. No SQL schema is specified here.

### Analytics

Python owns robust capacity estimation, median and weighted-median calculations, MAD-based outlier handling, percentiles, confidence calculations, history/trends, quota-change detection, task-size clustering, and benchmark analysis. Python does not collect Codex telemetry and is not the primary long-running tracker.

### Presentation

The Rust CLI is the v1 interface. It renders normalized application and estimator results; formatting does not own telemetry, domain semantics, storage, or estimation. The unreleased P034–P035 TUI follows the same rule: Ratatui rendering, application state transitions, bounded worker I/O, typed task-preparation state, and existing P031–P033 domain services are separated. It opens P031/P032 SQLite reports read-only, uses P033 App Server metadata, and never parses raw rollout payloads in the presentation layer. Composer drafts use a separate, explicit user-owned document store; they do not create a second usage ledger. A web dashboard is outside v1.

## Logical Rust components

These are logical boundaries, not mandatory crates.

| Component | Responsibility and ownership | May depend on | MUST NOT depend on |
| --- | --- | --- | --- |
| `core/domain` | Provider-independent identities, values, states, invariants, and ports | Minimal general-purpose libraries | Codex layouts/fields, SQLite rows, Python internals, CLI formatting |
| `runtime/agent` | Long-running coordination, lifecycle, checkpoints, shutdown/recovery, single-writer ownership | Domain and application ports; adapter interfaces | Analytics implementation or terminal presentation |
| `source-discovery` | Discover source instances and platform environment; select adapters | Domain ports and platform abstractions | Estimator/statistics or SQLite representation |
| `parser/ingestion` | Incremental safe parsing and normalized event production | Domain types and source abstractions | Observation analytics, CLI output, arbitrary persistence of raw payloads |
| `quota` | Acquire and normalize quota samples/window evidence | Domain types and source/platform adapters | Capacity inference or assumptions about undocumented allowances |
| `reconciliation` | Stabilization, reset-before-delta checks, per-window evidence classification | Domain state and normalized samples/events | CLI rendering, Python internals, source-specific JSON fields |
| `storage` | Rust-owned SQLite creation, migrations, repositories, transactions, checkpoints | Domain/application persistence ports and SQLite infrastructure | Presentation or statistical policy |
| `application/services` | Use-case orchestration, transactions, analytics invocation, result validation | Domain ports and injected infrastructure | Concrete terminal styling or Python module internals |
| `cli` | Commands, user input, rendering, exit behavior | Application services and presentation models | Direct telemetry parsing, SQL mutation, estimator algorithms |

Infrastructure implementations point toward domain/application interfaces rather than reversing the dependency. Boundaries should be tested without requiring every logical component to become a crate.

## Conceptual Python components

One future Python distribution may contain:

```text
codex_meter/
  estimator/   # capacity inference and orchestration
  statistics/  # reusable robust statistical primitives
  history/     # trends and metering-change detection
  benchmark/   # controlled-observation comparisons and calibration analysis
```

The package must be independently runnable and testable for research, but it neither watches telemetry nor becomes the runtime agent.

## Rust–Python data boundary and analytics invocation

The initial boundary is **SQLite plus versioned JSON contracts**, not in-process FFI:

1. Rust prepares a versioned request/context and invokes a Python analytics process using invocation details to be selected later.
2. Python opens only the allowed SQLite data read-only during normal estimation and computes statistics.
3. Python returns a versioned JSON result on the process boundary.
4. Rust validates the version and content before displaying it or optionally persisting a derived snapshot through Rust-owned storage.

Python must not write arbitrary derived data directly to SQLite. This gives Rust unambiguous migration and transaction ownership and avoids multiple-writer locking and schema ambiguity. PyO3/maturin or other FFI is intentionally deferred unless evidence establishes a need.

Malformed output is rejected before mutation. Python process failure is an explicit analytics error; it is not reported as tracking failure. Raw/normalized observation evidence remains durable when estimation fails or Python is unavailable, so collection can continue and estimation can be retried. Both sides can be tested independently with contract fixtures; Python can also be run independently for analysis.

## Observation lifecycle

```text
Detected → Active → TaskEnded → AwaitingMeter → Reconciling
                                                   ├─→ Finalized
                                                   ├─→ Incomplete/degraded
                                                   └─→ Invalid
```

**Task ended does not mean observation finalized.** Task completion closes local activity only; quota evidence may remain pending while a delayed meter stabilizes. States may also recover after restart where preserved evidence and deadlines allow.

Validity is not all-or-nothing. An observation can have valid token evidence, an invalid 5-hour capacity contribution due to reset, and a valid weekly contribution (or any corresponding combination). Quality/validity and reasons must therefore be expressible per measurement domain and quota window. An invalid quota contribution does not erase useful token evidence. P003 will define representations rather than this document preempting a schema.

## Reconciliation and reset architecture

Reconciliation belongs to the Rust runtime:

```text
task/session ends → capture final local tokens → sample quota
                  → compare with prior sample → detect reset before accepting delta
                                              → stable? ─yes→ finalize
                                                  │
                                                  no
                                                  ▼
                                               retry
                                                  │
                                  deadline / insufficient evidence
                                                  ▼
                                       incomplete or degraded
```

Retry intervals, deadline, and stabilization rules remain configurable and evidence-driven; P001 example timings are not architecture constants. The design must represent delayed and unchanged meters, multiple successive changes, temporary acquisition failure, concurrent external usage contamination, and reset crossings.

Reset detection occurs before accepting each quota delta and operates independently for 5-hour and weekly windows. A crossing invalidates only the affected quota-derived contribution where possible. A conceptual `QuotaWindow` identity distinguishes observations across resets without assuming an undocumented provider identifier. Evidence may include meter type, observed reset timestamp, sample timestamps, and a locally inferred window generation/identity. Its concrete shape belongs to P003.

## Dataset and configuration identity

Estimation uses normalized configuration identity that can distinguish at least plan, model, reasoning level, and speed mode. Codex version and observation time period are retained for segmentation when appropriate. Incompatible configurations or metering regimes must not be silently pooled. Every unavailable dimension is explicitly unknown—`speed = unknown`, for example—rather than guessed as `standard`.

## Time model

- Durable timestamps are UTC; local time is presentation-only where appropriate.
- Live durations use monotonic elapsed time so wall-clock adjustments do not corrupt them.
- Wall-clock evidence remains available for ordering and audit, with ambiguity represented rather than fabricated away.
- Quota reset timestamps/window evidence retain sufficient context to compare boundaries correctly.
- No Rust time library is selected by this architecture.

## Concurrency, idempotency, and crash recovery

### Single writer

One active Rust tracker/agent owns runtime writes. Read-only CLI queries may coexist, and Python should use read-only access. A second tracker must not silently create duplicate observations. Future process coordination/agent locking must claim sources and SQLite locking/busy behavior must be deliberate. Locking is a later implementation requirement, not implemented here.

### Idempotent ingestion

Filesystem notifications are hints, not unique events. Ingestion/storage operations should be idempotent wherever practical and preserve stable source, cursor/checkpoint, and event identity sufficient to avoid double-counting after restarts, replayed records, duplicate notifications, partial writes, re-reading, and reorderings where supportable. Checkpoints advance atomically with accepted evidence; incomplete records are not treated as complete. Rust telemetry and SQLite modules implement the current cursor and checkpoint design.

### Recovery

After restart, the runtime should recover persisted state, distinguish finalized from incomplete observations, resume safe ingestion, avoid duplicate accounting, and decide from timestamps/evidence whether reconciliation may continue. It preserves partial evidence and explicit unknowns rather than fabricating missing values or upgrading interrupted work to high-quality evidence. Transactional boundaries should prevent half-applied counter/checkpoint updates.

## Data ownership

| Data or concern | Primary owner | Rationale/boundary |
| --- | --- | --- |
| Codex telemetry acquisition | Rust | Runtime-local, incremental, platform-aware work |
| Raw telemetry parsing | Rust | Privacy filtering and normalized event production occur at ingress |
| Runtime task/session tracking | Rust | Single coherent lifecycle owner |
| Quota acquisition | Rust | Runtime evidence is reconciled transactionally |
| Reconciliation | Rust | Must coordinate live session and samples |
| Reset detection | Rust | Must precede acceptance of quota deltas |
| SQLite creation and migrations | Rust | One schema authority |
| SQLite runtime writes | Rust | One primary writer and transaction owner |
| Robust statistics | Python | Research-heavy, independently testable analytics |
| Capacity estimator | Python | Statistical inference, not runtime collection |
| Historical statistical analysis | Python | Analytical concern |
| Quota-change analysis | Python | Cross-observation statistical concern; Rust still records reset evidence |
| Benchmark execution/runtime | Rust | Controlled acquisition uses the normal runtime pipeline |
| Benchmark statistical analysis | Python | Distribution/effect/calibration analysis |
| User-facing CLI | Rust | Primary v1 interface and validated output owner |
| Durable estimator result persistence | Rust | Python returns JSON; Rust validates and writes if appropriate |

## Benchmark boundary

Rust owns controlled-run boundaries, configuration/environment metadata, telemetry collection, quota reconciliation, and Observation persistence. Python provides internal historical, workload, benchmark, capacity, and regime analyses. The `codex-meter estimate` CLI currently exposes only empirical current-capacity analysis; production benchmark execution and the other analytics CLI modes remain deferred. Benchmarking exists to improve measurement and estimation quality; it is not a general model leaderboard.

## Privacy and diagnostics boundary

Prompts, responses, source code, repository contents, Git remotes, user email, credentials, OAuth tokens, and access tokens MUST NOT enter persistent telemetry. When a source mixes allowed numerical metadata with prohibited content, the adapter/parser extracts only necessary allowed fields; prohibited fields are never copied into normalized events, SQLite, logs, analytics JSON, diagnostics, or benchmark records.

The P035 Composer may persist prompt text only after an explicit user save action, in a versioned,
per-user draft directory separate from telemetry SQLite. Draft files are plaintext and are not
encrypted by filesystem permissions. They are not autosaved, logged, synchronized, or included in
telemetry. Composer preflight is read-only and prepares a `PreparedTask` for the future P036
execution controller; it does not start a thread or turn.

Diagnostics may report sanitized metadata such as source type, normalized event type, timestamps, counters, errors, and offsets/cursors. They must not dump whole source records or payloads that may contain content. Debug mode follows exactly the same privacy contract. Troubleshooting features require privacy review and cannot bypass it for convenience.

## Unknown-source policy

The following remain explicitly unvalidated: exact Codex telemetry locations and JSON/JSONL formats; quota access; meter latency; reset metadata; availability of model, reasoning, and speed fields; and whether quota is shared across concurrent Codex/Work surfaces. Discovery work supplies evidence to interfaces/adapters. It must not invent APIs, fields, endpoints, filenames, or quota semantics, and discoveries must not force source details into the domain.

## Failure isolation

- Estimator or malformed-result failure cannot delete or corrupt observations.
- Weekly acquisition failure need not invalidate valid 5-hour evidence, and vice versa.
- A 5-hour reset crossing need not invalidate weekly or token evidence.
- If Python is unavailable, tracking/persistence can continue while estimation explicitly reports analytics unavailability.
- Temporary quota failure allows token tracking to continue with the affected quality/domain downgraded.
- Storage failure is explicit; it must not be papered over as successful collection.

Future schemas must preserve this granularity rather than collapse an observation into one validity bit.

## Dependency direction

```text
CLI → application services → domain/core ← infrastructure adapters
```

Application services depend on domain-owned abstractions; infrastructure supplies implementations. Core never depends on CLI formatting, SQLite details, Codex file layout, or Python internals. Cycles are prohibited. Python remains outside the Rust dependency graph and communicates only through the SQLite/read-only and versioned-JSON process boundary.

## Initial implementation strategy and deferred work

Logical modules do not imply a crate each. P004 should begin with the smallest practical Rust workspace/crate count and split only when boundaries become operationally useful. Start with one Python distribution/package, centralize future schema definitions, and avoid speculative plugin frameworks, FFI, distributed services, and daemon complexity beyond automatic local tracking needs.

P002 implements documentation only. It explicitly defers the Cargo workspace, Rust crates, Python package, SQLite schema and migrations, JSON Schema files, telemetry discovery, JSONL parser, quota provider, watchers, reconciliation runtime, estimator, benchmarks, TUI, web dashboard, and community upload. P003 is expected to specify versioned domain/data contracts and schemas; P004 is expected to establish repository/workspace and CI/tooling foundations.
