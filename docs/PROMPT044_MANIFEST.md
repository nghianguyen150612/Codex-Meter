# P044 manifest: P044A complete, P044B1 implemented

P044A is COMPLETE. P044B1 implements the in-memory qualified quota sampling
coordinator described below. P044B2 has a partial format-3 durable metadata
increment; workflow recovery and Observation finalization remain incomplete.
Overall P044 remains PARTIAL. Final-head hosted acceptance for P044B1 is tracked
in the dedicated section of Draft PR #39; source implementation alone does not
substitute for those checks.

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

### P044B2 durable checkpoint increment (partial)

Checkpoint state format 3 adds a bounded `qualified_sampling` metadata map to
`RuntimeRecoveryState`. It records source-generation, task/session and interval
identity, contract label, phase, validated policy, original deadline, explicit
evaluation time, and a bounded sample identity/digest ledger. The workflow key
hashes the whole source/task/session/interval association. The existing active
P012 reconciliation and policy remain the sole persisted P012 state; initialized
sampling metadata must match that state exactly. The payload has a 4 MiB cap,
128-workflow cap, 512-digest-per-workflow cap, and 256-byte identifier cap.

Version 1 and 2 checkpoint decoding remains supported. Missing legacy sampling
metadata is unavailable, and older formats cannot contain or promote qualified
metadata. The row format and embedded format must agree. Checksum validation,
revision CAS, and exact source-generation binding remain in force. File-backed
SQLite recovery tests exercise v3 close/reopen; source-generation mismatch is
rejected. A serialized `authority_contract` is descriptive metadata only: no
credential is serializable or restored, and production still has no issuer.

This increment does not yet make `TaskQuotaSampling` resumable: it does not
persist baseline sample payloads or restore the private coordinator evidence
ledger after a crash. It does not provide a durable workflow service, P013
terminal assembly, P044 provisional Observation ordering, or P044 atomic
terminal commit/cleanup. The existing P016 atomic Observation/checkpoint API is
unchanged, and historical terminal rows remain immutable. These gaps leave
P044B2 and overall P044 PARTIAL. P045 is not started; P038 G4/G5 remain
UNVERIFIED. PR #39 remains Draft and unmerged on the existing P043 base.

Focused checkpoint validation passed locally with Rust 1.100.0:
`cargo test --manifest-path rust/Cargo.toml -p codex-meter --lib
storage::checkpoints::tests` (15 tests), file-backed recovery included. The
workspace default and all-feature Rust test commands, workspace Clippy,
`cargo fmt --check`, Python pytest (677 tests), Ruff check/format, contract
validation (8 schemas, 19 positive and 7 negative fixtures), P029 installed
worker smoke, and Linux PTY recovery tests passed locally.

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
the bounded metadata increment only; they do not close the P044B2 workflow
integration gaps above.
