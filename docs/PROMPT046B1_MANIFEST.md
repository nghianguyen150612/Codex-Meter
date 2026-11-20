# P046B1: Controlled account quota snapshot comparison

## Scope and dependency

P046B1 is stacked on P046A, `feat/p046-source-qualification`, whose verified
baseline was `86fbaa4ae58491f7a3545ec05204c4f13b88c806`. It adds an explicit,
read-only comparison of exactly two saved P045B account snapshots. It does not
start P046B2 and does not qualify production source semantics.

## Windows CI baseline triage

The original push workflow `44672702389`, attempt 1, failed on Windows in the
all-features library tests at
`benchmark::runtime::tests::timeout_is_unknown_and_exact_replay_cannot_launch_again`.
The failing assertion checks that the cancellation token is set after a result
is classified as timed out. The code used `recv_timeout` to request
cancellation, but the worker-result path separately converted a late result
to `TimedOut` based on elapsed duration without requesting cancellation. On a
delayed Windows scheduler, a worker can queue its result after the deadline
before the waiting thread is rescheduled; `recv_timeout` then receives that
result and takes the path that omitted the signal.

An unchanged-head retry completed as attempt 2 of the same workflow and passed
the Windows all-features job, including the same test. This records both the
original failure and the successful retry; the retry does not erase the first
failure. A narrow fix now requests cancellation when a returned worker result
arrives beyond the deadline, matching the timeout receive path. The assertion
and the 1 ms timeout remain unchanged. A focused test verifies cancellation
for a late completed result.

## Comparison and storage contract

The CLI is:

```text
codex-meter quota --compare-live \
  --before-snapshot SNAPSHOT_ID --after-snapshot SNAPSHOT_ID [--database PATH] [--json]
```

The two names are distinct from history pagination's `--after` cursor. Both
IDs are required and are never reordered. Incompatible live, save, diagnose,
history, source, limit, and cursor flags are rejected. The explicit database
override selects an existing file. The implementation opens it using
`SqliteReadStore`, reads the two rows through the stored checksum decoder, and
does not write the database, apply migrations, update rollout state, save a
comparison, start App Server, or issue an RPC.

The evaluator validates the saved storage/schema version, fixed source and
provenance, account-level evidence scope, opaque snapshot ID syntax, UTC
timestamps, meter evidence, and strictly increasing acquisition times. The
comparison result has schema version `1.0.0`. It contains the two IDs and
times, fixed source and scope, bucket identity, source compatibility,
chronology, independent meter results, reset evidence, observed values,
movement only when justified, and typed warnings. It contains no account ID,
credential, prompt, raw RPC, path, or token/capacity estimate.

Five-hour and weekly meters are evaluated independently. A reported movement
requires both values to be observed, a main-bucket identity, matching observed
reset metadata whose boundary is still after both samples, and a non-decreasing
used percentage. A boundary crossing, a reset that changes without crossing
evidence, a reset time already at/before the baseline, a decreasing meter, or
missing reset metadata never yields movement. Missing and ambiguous meter
evidence is retained as such, never treated as zero. Equal observed values
yield a zero-point paired account observation, not meter stability and not
zero task consumption.

## P011 decision and evidence limits

P011 `QuotaTrackingState`/`QuotaTrackingOutcome` is not reused. Its normalized
input contract represents rollout/local-meter evidence; adapting saved live
App Server account snapshots into `QuotaSourceKind::LocalMeter` would change
their source provenance. P046B1 therefore uses a narrow pure pairwise
comparator. It requires provider-observed reset compatibility and does not
infer a local window from timestamp proximity. No reset timestamp is created.

Fixed source compatibility does not prove that two samples belong to the same
authenticated account. Account identity continuity is always marked
unverified. External account activity, task causality, controlled benchmark
isolation, token coverage closure, production quota accuracy, official token
allowance, and estimator eligibility are always explicit uncertainties. A
valid percentage-point movement remains account-level evidence and cannot
identify which task caused it. No quality A/B, P044 credential, qualified
quota event, P013 Observation, or estimator input is created.

The output is reproducible from the two stored IDs and evaluator schema
version. Comparison results are not persisted; no SQLite migration is added.

## Tests and validation

Dedicated tests cover independent and fractional five-hour/weekly changes,
unchanged values, missing and unavailable meters, reset boundary handling,
changed reset identities, conflicting bucket evidence, unsupported source and
scope, duplicate selections, and non-increasing timestamps. File-backed tests
store two synthetic acquisitions, compare through both human and JSON output,
verify exact stored checksum reads, reopen the database, compare file bytes
before and after, and check rollout and Observation tables remain unchanged.

Local validation on the implementation source passed:

- `cargo fmt --all -- --check` and workspace/all-target/all-feature Clippy with
  `-D warnings` passed.
- Default workspace/all-target tests passed: 614 library tests plus all
  integration targets. All-feature workspace/all-target tests passed: 612
  library tests plus all integration targets.
- The comparator, P011 tracker, P044 sampling coordinator, P045A rate-limit
  reader, P045B live snapshot storage, P046A diagnostic, and quota CLI parser
  regression selections passed.
- Python Ruff lint/format and pytest passed (678 tests).
- Contract validation passed (8 schemas, 19 positive fixtures, 7 negative
  fixtures).
- Installed-worker runtime smoke passed. Linux PTY resize and Unicode smoke
  passed.
- File-backed comparator tests verified checksum reads, no database-byte
  changes, and unchanged rollout rows and Observations.

Exact-head GitHub CI and release-candidate run URLs are recorded in the PR
closeout. No real account or quota-consuming task is used. Offline validation
does not qualify an authenticated live source, production accuracy, or
physical-device behavior.

## P046B2 and deferred qualification

P046B2 may add a user-controlled measurement-session workflow with an explicit
session start, user-confirmed before and after acquisitions, bounded elapsed
time, and explicit overlap and uncertainty reporting. The measurement service
must not execute tasks or poll in the background and must not promote results
automatically into estimator inputs. That workflow is not part of P046B1.

Authenticated source semantics and P038 G4/G5 physical-device qualification
remain **UNVERIFIED**. P046 overall remains **PARTIAL** until P046B2 and the
independent qualification gates are addressed.
