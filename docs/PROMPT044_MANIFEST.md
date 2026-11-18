# P044 manifest

P044A, P044B1, and P044B2-R1 are implemented. P044B2-R2 implements the
terminal Observation transaction and file-backed crash recovery. The
[P044B2-R2 closeout in Draft PR #39](https://github.com/nghianguyen150612/Codex-Meter/pull/39)
records the engineering status and exact-head hosted validation. Live-source
qualification remains blocked, and P038 G4/G5 remain unverified.

## Pinned source evidence

The supported compatibility baseline is Codex CLI 0.157.1, audited against
upstream revision `8f7a0f7a878199c6886600370e5be6bd37ca38a3` (see
`V02_REAL_TELEMETRY_INGESTION.md`). At that revision:

- `codex-rs/protocol/src/protocol.rs`, `TokenUsageRecord`, explicitly calls
  the per-response usage best effort. It has response and turn identities and
  separate response, turn, and thread usage fields.
- `codex-rs/core/src/session/turn.rs`, `ResponseEvent::Completed`, passes
  optional usage to `record_observed_response_completed`.
- `codex-rs/core/src/session/mod.rs`, `record_observed_response_completed`,
  persists a `TokenUsageRecord` only when usage is present. A completed response
  without usage therefore need not yield such a record.
- `TurnCompleteEvent` in `protocol.rs` contains turn identity and optional
  timing, but no response count or token coverage watermark. Its terminal event
  alone does not establish exhaustive response usage coverage.
- `TokenCountEvent` carries optional aggregate usage and rate-limit snapshots;
  `send_token_count_event` reads the current session state. In `turn.rs`, a
  rate-limit update can cause this event to be emitted. This is not evidence of
  a qualified before/after task sampling protocol or of exclusive task usage.

These observations do not establish exhaustive stream completion, a last
canonical token event, generation continuity, task-level source closure, or
sampling stability. The audit has not established a task-associated baseline or
post-task quota sample. More source and recovery analysis is needed before any
new proof can be accepted. Existing P043 incomplete classification remains the
default; the presence of individual response usage does not change it.

## Baseline

P043 HEAD was `a2c5cab2521b9831c47cd4e104f9fc29ab1f6495`; its push and PR CI
and release-candidate validation passed. These runs do not validate P044.
P043 persists terminal observations without task-associated quota samples;
P044 must resolve lifecycle ordering and crash recovery without modifying those
historical terminal records.

## P044A implementation

`telemetry::coverage` separates reported source continuity from task coverage.
It assesses identity, boundary presence, lifecycle anomalies, mixed configuration,
and canonical event identity and duplicate conflicts. Its status distinguishes
unavailable, partial, continuous but unproven, and conflicting evidence. Verified completeness requires a private, non-deserializable closure proof with
a recognized contract and provenance, matching source generation, task and
session identities, committed nonzero boundary, and exact canonical event-ID
membership. Conflicts, missing evidence, and discontinuity fail closed. The
only recognized authority is compiled exclusively into unit tests; production
rollout ingestion cannot construct or supply a proof. Assessment fields are
private so callers cannot fabricate a favorable result. Continuity supplied by a caller is
not closure, and the recorder uses unknown continuity because a cursor alone
cannot prove the whole task history. The boundary passed at assembly is a
candidate committed atomically with the observation and checkpoint, not an
independently durable guarantee.

The recorder continues to select canonical per-response records from the current
source generation and delegates checked accounting and deduplication to P013.
Snapshots are excluded by that existing selection; raw totals and missingness
are unchanged. The assessment is transient and introduces no migration or
historical row rewrite. P043 checkpoint and terminal immutability rules remain
in force. Unit tests cover continuity without closure, missing boundary/start,
lifecycle conflicts, identity, duplicate replay and conflicting duplicate data.
Existing sync and storage regression tests exercise the recorder, checkpoint
replay, source replacement, incomplete tails and terminal immutability.

No production source has an audited closure adapter. The synthetic authority
exercises the admission logic without changing this limitation. Dedicated tests
cover matching and mismatched proof identities, membership, boundary and
authority, duplicates, discontinuity and lifecycle conflicts. A sync regression
checks a task across process restarts and later token evidence against immutable
terminal JSON; the failed-transaction regression checks the checkpoint offset,
revision and state checksum remain unchanged. P044B must implement
explicit task-associated quota samples, P012 qualification, bounded sampling,
and atomic durable pending reconciliation without upgrading terminal rows.

### Trusted closure contract and authority

The trusted closure implementation was introduced in commit
`c497818650fda1eb31eae50e9a24b55dfaa2a9a0`. Its private proof carries the
contract version (encoded by the private contract variant), source generation,
task and session identity, committed boundary, exact canonical response-ID set,
and provenance. The only constructor is internal. The synthetic authoritative
fixture is compiled only for unit tests and is not reachable through
`codex-meter sync`, runtime flags, or input JSON. Production rollout assessment
always enters without a proof and remains incomplete.

`VerifiedComplete` requires a recognized contract/provenance pair; a matching
source generation, task, session, and nonzero committed boundary; exact response
membership; continuous source evidence; complete task lifecycle evidence; and
no identity, configuration, duplicate, or lifecycle conflicts. Missing proof
remains unproven. Malformed proof, mismatched membership/boundary/identity,
source discontinuity, and lifecycle conflict fail closed. Only an assessment
created by this validation can map token completeness to `Complete`.

The focused closure suite covers valid synthetic proof; missing proof; wrong
generation, session, and task; missing expected and extra observed responses;
identical and conflicting duplicates; candidate/committed boundary mismatch;
discontinuity; conflicting lifecycle and post-terminal tokens; malformed
authority; and production rollout inability to promote coverage. Recovery
regressions cover an incomplete record recovered in a later batch, checkpointed
task state across process restarts, idempotent repeated sync, rollback of cursor
state after transaction failure, and immutable terminal observations after
later token evidence.


## Closeout validation

Commit `c497818650fda1eb31eae50e9a24b55dfaa2a9a0` changes Rust source and this
manifest beyond the prior P044A baseline
(`698160f5046517f8133320bf8e2ef9093aa268eb`). Final-head CI and release
validation links and results are recorded in PR #39. At `c497818`, push CI
([run 43181901696](https://github.com/nghianguyen150612/Codex-Meter/actions/runs/43181901696)),
PR CI
([run 43181910298](https://github.com/nghianguyen150612/Codex-Meter/actions/runs/43181910298)),
and four-platform release candidate validation
([run 43181924544](https://github.com/nghianguyen150612/Codex-Meter/actions/runs/43181924544))
passed. The local Rust, Python, contract, installed-worker, and Linux PTY results
for the follow-up regression additions are recorded with their final-head checks
in PR #39. The earlier baseline CI at `698160f` is not evidence for validating
this implementation. Overall P044 remains partial pending P044B.


## P044B1: qualified in-memory quota sampling

Continues `feat/p044-task-coverage-quota-evidence` from
`6798fe1962f06782d2765bf245bce40f28645dd1`, preserving all P044A commits.
`telemetry::task_quota_sampling` adds `TaskQuotaSampling` with typed registration,
before-sample admission, initialization, explicit-time advancement, read-only
P012 batches (states, outputs and pending actions), and completion inspection.
Admissions distinguish accepted evidence from replay; failures return typed
rejections. Registration binds contract version, normalized task/session and
source generation, observed lifecycle interval, full validated policy identity,
authority contract and phase. The full policy value is its identity, rather than
an unverified caller-selected policy name.

`AssociationClaim` is untrusted data. `AssociationCredential` and
`QualifiedQuotaEvent` have private fields, no public constructors, and no serde
implementation. Only the private unit-test synthetic provider issues them.
Recognition of the synthetic authority is itself compiled out of production;
production builds recognize no authority. Rollout imports, timestamps, task IDs,
JSON, flags and environment variables cannot create a capability or promote an
account snapshot. A separate integration test links the production library and
verifies uncredentialed registration remains unavailable; compile-fail doctests
check private construction and JSON deserialization boundaries. Synthetic
fixture samples are not real provider observations.

The coordinator forwards admitted baselines to
`begin_task_quota_reconciliation` and acquisition attempts to
`reconcile_task_quota_at`. P012/P011 remain the only baseline selection, tracking,
delta, reset, stability, confirmation and deadline engines. There is no clock
read, polling loop, thread, provider adapter or independent delta calculation.
Batch ordering is P012's existing ordering contract; streaming late arrivals
retain P012's out-of-order behavior. Evaluation time cannot move backwards.
Identical evidence is forwarded to P012's deduplication (including future
samples replayed at a later evaluation time). A meter-scoped identity ledger
adds only payload/provenance conflict detection, which P011/P012's seen-ID sets
do not provide. Validation and P012 errors leave the entire batch uncommitted.
New evidence targeting a terminal meter is explicitly rejected; identical
terminal replay remains idempotent. Read-only reconciliation access prevents
callers from changing established results.

Five-hour and weekly baselines, windows, outcomes and missingness stay
independent. Verified stable zero remains zero; unavailable evidence is never
manufactured as zero. Known local overlap is preserved without allocating
account movement between tasks. `NoKnownLocalOverlap` proves no account-wide
isolation; external clients and cloud usage remain unproven. No isolation grade
A or B is awarded, and a positive delta is not exclusive task consumption.

### Dedicated and regression validation

The 32 dedicated unit tests cover valid and missing authority, unsupported
production authority/contract/version, task/session/generation/policy/interval
mismatch, phase and meter mismatch, baseline selection/missing start/staleness,
positive and zero deltas, independent availability/reset/failure/timeout,
plan discontinuity, instability, threshold/confirmation rules, explicit
deadlines, replay/conflict handling, terminal updates, equivalent batch orders,
streaming stale/future samples, atomic rejection, overlap and external
uncertainty. The production boundary integration adds one test; three
compile-fail doctests exercise capability construction/deserialization.

Local validation includes Rust format and Clippy (all targets/features, warnings
as errors), default and all-feature workspace tests, Python pytest (677 tests),
Ruff lint/format (35 files), contract validation (8 schemas, 19 positive and
7 negative fixtures), installed-worker runtime smoke, and Linux PTY regressions.
Exact workspace totals, final-head SHA, push/PR CI and four-platform release
validation URLs and acceptance status are recorded in PR #39. Earlier P044A
workflow runs are not validation evidence for this new source.

### P044B2-R1: resumable qualified sampling

`TaskQuotaSampling::snapshot()` exports format-3 workflow metadata, normalized
baseline samples, accepted normalized acquisition events, sample digests, and
the existing active P012 reconciliation when initialized. Format-3 event
payloads are bounded and validated; provider bodies and authority credentials
are never stored. Accepted events later than an evaluation time remain pending
in the ledger and are replayed through P012 when their timestamps become
eligible, preserving replay/conflict behavior across restarts.

`TaskQuotaSampling::recover()` validates a snapshot into an inspectable,
unauthorized value. `reauthorize()` requires a fresh recognized
`AssociationCredential` and compares authority contract/version, source
generation, task/session, interval, policy, and workflow identity. Checkpoint
bytes, checksums, and matching source IDs do not grant authority. Production
continues to recognize no issuer; the synthetic authority remains test-only.

Initialized recovery reuses `RuntimeRecoveryState.active_reconciliations` and
P012's `validate_recovered()` contract. It preserves both meter states,
confirmation progress, reset/window evidence, evaluation time, deadline, and
actions. No quota delta or meter state is independently reconstructed. The
composite workflow key does not merge same-task workflows with different
session/interval/source identities. Since active P012 state remains keyed by
task ID, a conflicting initialized workflow fails with a typed identity error.

Version 1 and 2 checkpoint decoding remains supported with absent qualified
metadata unavailable. Row and embedded formats must agree. The 4 MiB,
128-workflow, 512-event/sample-identity, and 256-byte identifier limits remain
in force. `save_sampling_snapshot()` preserves the current cursor and committed
source boundaries, merges one workflow transition, and uses the existing
SQLite immediate transaction and revision CAS. Identical replay is idempotent;
failed validation or CAS leaves storage and the live coordinator unchanged via
`transition_and_persist()`.

File-backed tests close and reopen between registration, accepted baselines,
and initialized reconciliation. Coordinator tests cover a future accepted
event becoming eligible after restart and fresh-authority revalidation.
The R2 handoff below covers terminal Observation assembly and atomic cleanup.
P038 G4/G5 remain unverified. PR #39 remains Draft and unmerged on the P043
base.

R1 implementation validation is recorded with its tested commit and exact
counts in the PR update. Earlier metadata-only P044B2 validation below does not
validate R1.

Hosted validation passed at source SHA
`d12da09bc2ee3ef46326548cf11259469e60f477`: push CI
([run 44005529010](https://github.com/nghianguyen150612/Codex-Meter/actions/runs/44005529010)),
PR CI
([run 44005535586](https://github.com/nghianguyen150612/Codex-Meter/actions/runs/44005535586)),
and four-platform release candidate validation
([run 44005570561](https://github.com/nghianguyen150612/Codex-Meter/actions/runs/44005570561)).
CI passed on Linux, Windows, macOS Intel and macOS ARM64, including ConPTY and
the workspace default/all-feature suites. Release bundle generation, installed
runtime smoke, aggregation, and checksum/provenance verification passed for
Linux x86_64, Windows x86_64, macOS x86_64, and macOS ARM64. These runs validate
the bounded metadata increment only; they do not validate P044B2-R1 or R2.

## P044B2-R2 terminal finalization

`SqliteStore::finalize_sampling` takes an authorized coordinator and a
`SamplingFinalizationRequest` with the expected normalized source, task,
session, interval, workflow ID and SQLite revisions. The authorized policy
and the original deadline must agree with the persisted workflow and P012
state. P012 must have a terminal outcome for each meter and no remaining
actions other than its completion marker.

The finalizer supplies P012 state directly to P013. Five-hour and weekly
evidence stay independent, including observed zero, reset, timeout and
unavailable evidence. Normalized per-response events may supply observed token
counts, but R2 has no P044A closure proof and keeps coverage incomplete or
unavailable. Source fallback uses the independently validated normalized source
association. Isolation is unknown and does not qualify for A or B.

Pending work stays in the runtime checkpoint. The immediate transaction uses
P016 to insert or legally advance the existing Observation under revision CAS.
It replaces the pending workflow with a format-4 completion receipt, removes
its active P012 state if unshared, and updates the checkpoint checksum and
revision. The source cursor and boundary are preserved. An identical terminal
Observation can be adopted at its current revision. A differing terminal
payload or source generation produces a conflict, preserving both the existing
Observation and the pending result.

A stop before commit leaves the pending checkpoint. After commit, the
checksummed receipt retains normalized terminal replay material and the
Observation payload checksum. `recover_finalized_sampling` returns
unauthorized replay data for the exact workflow. After reauthorization, the
same evidence and the original or freshly loaded revisions can acknowledge
the commit without another write. A missing pending entry alone, a missing
Observation, mismatched evidence, and attempts to restore completed work fail
closed. The receipt contains no credential.

File-backed regressions exercise stable and zero deltas, asymmetric meter
validity, reset, timeout, acquisition failure, pending and identity rejection,
both revisions, provisional update rollback, cursor and boundary preservation,
collision handling, immutable insertion replay, and restart before and after
commit. The suite compares stored payloads, revisions and checksums.

### macOS Intel baseline triage

The original Intel push job on
`648f124fc27d8b35e62097c296c1f26cd72f2ed8` failed with the reported
Python-worker pipe-lifecycle error in the environment allowlist test. The
equivalent PR job passed. The unchanged-head hosted Intel rerun passed the
default and all-feature suites. [Original push job](https://github.com/nghianguyen150612/Codex-Meter/actions/runs/44015249012/job/133811500898)
· [PR job](https://github.com/nghianguyen150612/Codex-Meter/actions/runs/44015255408/job/133811522254)
· [Hosted Intel rerun](https://github.com/nghianguyen150612/Codex-Meter/actions/runs/44015249012/job/134531680578)

The Actions log download was unavailable from this workspace. The hosted
rerun did not reproduce the failure. Root cause remains unconfirmed.
Final-head Intel CI explicitly runs the focused test with the original
worker timeout, pipe checks, and security settings.

The test-only synthetic issuer remains the sole qualified sampling authority.
Production sync, statistical eligibility, and release/hardware qualification
are unchanged. Final-head push CI, PR CI and four-platform release evidence
are recorded by SHA in the Draft PR closeout.
