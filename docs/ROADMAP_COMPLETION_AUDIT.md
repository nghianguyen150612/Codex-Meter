# P001–P030 roadmap completion audit

This audit follows the milestone ranges and results in the authoritative
[roadmap](ROADMAP.md). A completed prompt means its recorded implementation work exists; it does
not mean every internal capability is part of the public CLI or has been validated against live
provider data.

The corrective milestones included in the audit are P003A, P009A, P012A, P013A, P016A, and P020A.
Their manifests record schema, telemetry, quota tracking/reconciliation, persistence, and estimator
corrections. They do not add separate user-facing commands.

| Phase | Milestones | Required result | Implemented internal capability | CLI-integrated capability | Production-validated capability | Deferred capability |
| --- | --- | --- | --- | --- | --- | --- |
| A — Foundation | P001–P004, P003A | Product contract, architecture, versioned schemas, repository and validation foundations | Product boundaries, architecture, schemas, fixtures, and corrective contract changes are recorded in the P001–P004 manifests and `schemas/v1/`. | No end-user command was required in this phase. | Contract/schema validation uses local fixtures; no provider-facing behavior was production-validated. | Future products and automatic integrations remain subject to the product contract. |
| B — Telemetry | P005–P009, P009A | Discover telemetry shapes; model sessions; parse rollout JSONL incrementally; normalize token evidence | Discovery records, rollout readers, incremental parsing, token normalization, session assembly, quota window tracking, and corrective parsing/identity work exist in `rust/crates/codex-meter/src/telemetry/` with synthetic fixtures. | These components are not automatically started by the installed CLI. | Parser behavior is fixture- and test-validated. Automatic collection from user sessions is not production-validated. | Tracker startup, daemon operation, and automatic session discovery in the released command. |
| C — Quota measurement | P010–P013, P012A, P013A | Model quota samples, resets, reconciliation, and observation correctness | Quota tracking, normalization, reconciliation, reset handling, and observation quality policies are implemented in telemetry modules and documented in the quota guides/manifests. | The estimate CLI accepts optional manually entered meter percentages; it does not acquire live percentages. | Synthetic sequences and local tests validate modeled behavior. Live provider quota acquisition and provider-attested correctness are not validated. | Automatic quota acquisition, refresh, and reconciliation against a provider account. |
| D — Storage | P014–P016, P016A | Persist observations through versioned SQLite migrations with validated projections | SQLite migrations 0001–0004, observation storage, migration checksums, checkpoints, and corrective persistence behavior are implemented. | The estimate command reads an existing compatible database through the read-only Python reader. Other storage APIs remain internal. | Synthetic database tests cover migration checksums, read-only behavior, and unchanged bytes. No production database was used. | Automatic population of the database and additional user-facing storage commands. |
| E — Estimator | P017–P020, P020A | Read historical observations, derive meter-specific candidates, aggregate robustly, report confidence-aware capacity | Read-only observation loading, candidate derivation, robust statistics/outlier handling, confidence, sample accounting, and current-capacity result construction are implemented in Python. | The current-capacity estimator is exposed through `codex-meter estimate` with AnalyticsRequest v1 and AnalyticsResult v1. | Representative and synthetic regressions, including the P029 output digest, validate the implementation. No live quota claim or user-history validation is made. | Automatic current-meter discovery and provider quota calibration. |
| F — History and prediction | P021–P023 | Analyze historical capacity, estimate remaining workload, signal possible metering-regime changes | Historical trends, task-size/remaining-workload prediction, and conservative regime-change signals are implemented in `python/codex_meter/analysis/` with separate versioned models. | These analyses are not exposed as CLI commands. | Synthetic and unit tests validate algorithms; no longitudinal production dataset or provider-attested outcome was used. | History, workload, and regime CLI commands and live calibration evidence. |
| G — Benchmarks | P024–P026 | Define controlled benchmark lifecycle, authorization/runtime boundaries, and statistical analysis | Benchmark planning, evidence, lifecycle, dispatch/runtime safeguards, and comparison/calibration analysis are implemented internally. | No benchmark command is exposed by the candidate CLI. | Framework behavior is unit-tested with synthetic records. Production benchmark execution and provider-attested calibration have not occurred and remain disabled. | Live benchmark execution, provider-backed calibration, and benchmark reporting through the CLI. |
| H — Integration and hardening | P027–P030 | Integrate the estimator CLI; preserve privacy and security; verify OS/runtime compatibility; prepare versioned distribution artifacts | P027 integrates the estimator; P028 hardens worker/process boundaries; P029 validates installed-worker compatibility; P030 adds synchronized versions, four candidate targets, integrity checks, installation docs, release notes, and this audit. Evidence is in the corresponding manifests and CI workflows. | Only `codex-meter estimate` is integrated. It requires an existing database and an installed compatible worker when distributed. | CI validates synthetic local behavior, Linux/Windows/macOS binaries, Python 3.11–3.13 runtime cells, and candidate extraction. A candidate is not a public release or live provider validation. | Automatic tracking, live quotas, daemon monitoring, additional CLI analyses, production benchmarks, signatures/notarization, and public publication. |

## Completion interpretation

- **Implemented internal capability:** source modules, contracts, and tests exist in the repository.
- **CLI-integrated capability:** callable through the installed `codex-meter` command. Currently
  this means current-capacity estimation only.
- **Production-validated capability:** validated against its intended live production source or
  operation. P001–P030 add no live provider or real-user-data validation; none is claimed here.
- **Deferred capability:** documented as a future roadmap candidate and not implied by completion
  of its implementation prompt.

P030's artifact status is conditional on its required checks. `docs/RELEASE_READINESS.md` defines
`READY_FOR_MANUAL_RELEASE_REVIEW` and `BLOCKED`; the final completion report records the actual
workflow results and the post-merge candidate source SHA. Even a passing status does not authorize
publication. No P031 work is included.
