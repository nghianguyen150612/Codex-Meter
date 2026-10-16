# Authorized benchmark runtime

## Runtime version and scope

P025 runtime behavior is versioned by `BENCHMARK_RUNTIME_VERSION = "1.0.0"`.
The P024 `BENCHMARK_FRAMEWORK_VERSION` remains `1.0.0`. P025 does not change
estimator, workload, or regime-detector versions.

The runtime coordinator owns preflight orchestration, one-run authorization
consumption, durable dispatch reservation, bounded adapter waiting, and
Observation correlation. It does not calculate quota deltas, infer capacity,
finalize Observations, or change their quality.

## Orchestration and adapter contracts

The Rust API is under `codex_meter::benchmark`:

- `BenchmarkRuntimeCoordinator` evaluates P024 preflight, requests a trusted
  per-run authorization, reserves one attempt, and coordinates Observation
  linkage.
- `BenchmarkRuntimeEvidenceSource` supplies typed runtime evidence. Its
  successful snapshot constructor is restricted to this crate; external code
  can use the safe all-unknown snapshot but cannot manufacture verified gates.
- `BenchmarkExecutionAdapter` validates an exact registry-resolved
  `WorkloadKey` and receives a bounded `ExecutionControl`. No shell command,
  prompt, response, or arbitrary environment map crosses the interface.
- `BenchmarkWorkloadResolver` must resolve a declared key through a trusted
  registry. Unknown keys fail before reservation.
- `BenchmarkDispatchGuard` is sealed to the Rust crate. The implementation is
  `FileBenchmarkDispatchGuard`, which uses the caller-selected journal
  directory.
- `BenchmarkObservationResolver` returns a sealed
  `ResolvedAuthoritativeObservation`. The provided SQLite resolver reads an
  existing `SqliteStore` record; it cannot create one.

The adapter interfaces are contracts for later integrations. P025 does not
include a Codex provider adapter, provider process launch protocol, provider
authentication flow, or provider workload registry.

## Execution capabilities

`ExecutionMode` names `Disabled`, `Simulation`, and `Live`:

- **Disabled** is the only production coordinator constructor. It returns
  `ExecutionDisabled` before consuming authorization or invoking an adapter.
- **Simulation** is available only to crate unit tests. The test adapter uses
  in-process fake events and never uses credentials, network access, Codex
  CLI, provider quota, or the production Observation store.
- **Live** has no production constructor. P025 does not implement a trusted
  user-action surface, a production authorization ingress, or a verified
  execution adapter, so live execution is unavailable.

Constructing P024's public `ExplicitRunAuthorization` is not a user action.
P025 requires the sealed `TrustedRunAuthorization` returned from a trusted
ingress. Its successful constructor is test-only in this version. A future
application must implement a real trusted ingress before it can request a live
run.

## Preflight evidence

`evaluate_preflight` asks an injected `BenchmarkRuntimeEvidenceSource` for
evidence, builds P024 `BenchmarkPreflight`, and records it through the existing
P024 lifecycle. Configuration readiness compares plan, model, reasoning
level, speed mode, and Codex version while preserving availability and
provenance. A known mismatch blocks. Any unavailable dimension remains
unknown; it is never treated as a match.

The P024 preflight includes telemetry source readiness, each selected meter's
availability, each selected meter's window continuity, isolation, and
execution-boundary readiness. `BenchmarkPreflight` enforces exact per-meter
coverage. Unknown meter evidence is not zero usage, and an unknown window is
not continuous. Five-hour and weekly evidence stay independent. A ready
preflight is technical readiness only; it does not authorize execution.

P025 has no production telemetry or quota-evidence source. The public helper
`RuntimeEvidenceSnapshot::unknown_for_plan` preserves that limitation. Tests
may use `AuthorizedTestFixture` evidence in Simulation; the coordinator rejects
that evidence if a future internal Live path is added.

## Authorization and one-run dispatch

One authorization request is scoped to an experiment, run, repetition index,
and one intended invocation. The coordinator accepts one run per call and
never expands one authorization into a batch. P024 enforces run limits and
rejects authorization reuse across runs. Cancellation before authorization
does not consume the event; after P024 accepts the event, cancellation does not
restore it.

The safe order is:

1. P024 preflight is ready.
2. The trusted ingress consumes the exact user action.
3. P024 accepts that scoped event.
4. The exact workload key and adapter are validated.
5. The caller-selected journal atomically reserves the scope.
6. The coordinator acquires a cross-process active-dispatch slot.
7. P024 records execution start, then the adapter is invoked once.

The durable identity is a SHA-256 digest of length-framed
`experiment_id`, `run_id`, `repetition_index`, and `authorization_id`. The
journal also stores those bounded opaque IDs so it can verify the digest. A
separate atomic authorization index binds each authorization ID to exactly one
scope, including across restarts. The index is written before the reservation;
if a crash leaves only the index, recovery reports uncertainty and refuses
reuse. The digest does not include timestamps, prompts, or source content.

## Durable journal and recovery

The Rust-owned journal format is `BENCHMARK_DISPATCH_JOURNAL_VERSION =
"1.0.0"`. Authorization-index, reservation, acknowledgment, and terminal
records are strict JSON files created with atomic `create_new`, written and
`sync_all`'d before adapter invocation.
Unknown fields, unsupported versions, invalid identities, digest mismatches,
oversized records, and I/O errors fail closed. Each record is limited to 4 KiB
and the journal accepts at most 10,000 reservation, authorization-index, and
Observation-claim records.

Reservation, acknowledgment, and terminal markers are separate immutable
records. Recovery reports:

- `NotReserved` when there is no reservation, authorization index, or stray
  state marker;
- `ReservedOutcomeUnknown` when an authorization index or reservation exists
  without acknowledgment;
- `DispatchAcknowledged` after an acknowledgment marker;
- `Terminal(outcome)` after a terminal marker.

A crash after reservation but before invocation, or after an invocation whose
acknowledgment was not persisted, is uncertain. The coordinator never
re-dispatches from recovery and never regenerates authorization. Exact replay
of a reserved scope conflicts. No stale reservation is deleted automatically.

One active-dispatch slot per journal directory serializes invocations across
threads, coordinator instances, and processes. It uses another atomic
create-if-absent file. Normal completion releases the slot. A crash or I/O
failure that leaves the slot behind blocks later dispatch until an operator
reviews the journal; absence of a provider process does not clear it. A
filesystem failure while persisting the reservation prevents adapter
invocation.

## Timeout and cancellation

`RuntimeExecutionPolicy` requires a nonzero maximum elapsed duration and
maximum evidence-wait duration, each no greater than 24 hours. The adapter
worker is waited on with `recv_timeout` using monotonic `Instant` timing. The
UTC evidence clock is queried at each state transition.

If the execution deadline expires, the coordinator returns an
`OutcomeUnknown` receipt, records timeout, and never retries. Under
`RequestWhenSupported`, it signals cancellation. Under `StopWaitingOnly`, it
stops waiting without claiming cancellation. Both policies close the
cancellation gate for a worker that has not crossed the adapter boundary, so
it cannot start after the timeout response. A timed-out worker retains the
active-dispatch lease until it returns; another invocation cannot overlap in
that journal. A worker that never returns leaves the slot unavailable, which
is fail-closed.

Cancellation before adapter invocation prevents the call and records a
terminal `CancelledBeforeDispatch` reservation when reservation already
exists. During execution, cancellation is only confirmed if the adapter says
it stopped. An uncertain cancellation remains uncertain. Neither cancellation
nor timeout claims that quota use was reversed.

Observation reconciliation makes one bounded resolver call per request; it
does not poll in a loop. If no terminal record is available, the run remains
`AwaitingObservation` and a later caller may ask again. This lookup never
launches a workload.

## Observation correlation and claims

An Observation ID supplied alone is not accepted. A crate-issued
`ExecutionCorrelationEvidence` must bind the experiment, run, repetition,
Observation ID, actual task ID, session ID, and source-instance ID. Its
constructor is crate-private for a future validated adapter. The SQLite
resolver loads that exact existing ID, and the coordinator independently
checks the Observation's derived identity and exact task/session/source
identity before it can call P024's crate-private
`TrustedRuntimeBinding::from_trusted_runtime` constructor.

P024 then checks terminal lifecycle, configuration, timing, target-meter
evidence, and link scope. A reconciling Observation remains unresolved. The
coordinator does not create a synthetic Observation, infer linkage from a
timestamp or token total, or write an Observation.

All coordinator instances in one process share one P024 claim ledger, bounded
to 10,000 claims. The filesystem journal also persists one owner per
Observation using atomic create-if-absent. The durable claim is written before
P024 commits completion. A conflicting owner blocks linkage; a same-owner
replay is idempotent. Cross-process claim uniqueness requires all coordinators
that can see the same Observation to share the same caller-selected journal
directory; distinct journal directories do not coordinate claims. P025 has no
production execution or recovery service that could establish a broader
claim domain. After restart, claim ownership can be inspected and preserved,
but P024 does not persist experiment lifecycle state. If a run cannot be
restored with sufficient lifecycle evidence, the runtime fails closed rather
than completing it.

## Isolation, quality, and per-meter integrity

The coordinator defaults isolation to `Unknown`. It does not infer isolation
from a lack of detected local processes, a controlled label, authorization,
preflight readiness, a workload key, or process success. P025 never assigns
`QualityGrade::A` or `IsolationEvidence::ControlledBenchmark`; only the
existing Observation pipeline owns quality. Persisted Observation quality is
never modified by runtime linkage.

Five-hour and weekly meter evidence remains authoritative in the Observation.
A reset or unavailable state in one meter does not change the other meter's
validity, reset status, raw totals, or quota delta. P024's aggregate
complete/incomplete result retains its existing meaning.

## Failures and privacy

Runtime failures are typed and privacy-safe, including disabled execution,
adapter/workload unavailability, unready preflight, authorization scope
mismatch, dispatch conflict or persistence failure, timeout, cancellation,
unknown execution outcome, pending Observation, unverified correlation,
duplicate Observation claim, and incomplete recovery evidence. Errors do not
include provider response bodies or raw event dumps.

Journal and receipt metadata contains only bounded IDs, repetition index,
status, UTC timestamps, and correlation state. It never stores prompts,
responses, commands, credentials, environment variables, model reasoning,
source content, token totals, or quota deltas. P025 adds no Rust or Python
runtime dependency, SQLite migration, schema, Python write path, CLI, scheduler,
or benchmark statistical calculation.

## What works today and remaining production boundaries

Today the library can evaluate unknown/test preflight evidence, exercise a
single fake adapter in unit tests, persist and recover reservations, reject
replays, enforce bounded worker waits, resolve existing Observations through
the SQLite adapter, and test trusted-correlation and claim behavior with
synthetic fixtures. These tests are not provider executions or quota
measurements.

There is no functioning production provider adapter. There is also no trusted
user-action ingress or production preflight evidence source. Consequently the
only production execution mode is Disabled. Live benchmark execution is not
enabled or tested. The future integration must add those verified boundaries
inside the Rust crate, keep the public coordinator disabled until they are
complete, and retain the same no-retry and fail-closed recovery rules.

P026 owns statistics and calibration. No P026 work is included here.

## Inputs for P026

P026 may assume:

* P024 defines validated benchmark experiments and lifecycle.
* P025 provides authorized runtime orchestration and explicit dispatch safeguards.
* Simulation and real execution evidence are distinguished.
* Dispatch replay cannot silently trigger duplicate workloads.
* Runtime uncertainty is preserved.
* Trusted Observation association is required.
* Per-meter validity and quality remain authoritative.
* Grade A is never assigned from benchmark labels.
* Historical benchmark evidence can be identified through validated run-to-Observation linkage where available.
* Rust continues to own execution and measurement.
* Python remains responsible for statistical calibration.

P026 will implement:

Controlled benchmark comparison, calibration analysis, statistical evidence reporting, and estimator-quality assessment.
