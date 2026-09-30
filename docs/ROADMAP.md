# Codex Meter Roadmap

This roadmap organizes the current approximately 30-prompt implementation sequence. Exact prompt boundaries MAY be refined as implementation and telemetry-discovery evidence emerges. The product scope and measurement semantics established in [the P001 product contract](PRODUCT_CONTRACT.md) MUST NOT drift silently; any superseding decision MUST be explicit and documented.

## Phase A — Foundation (P001–P004)

- product contract;
- architecture;
- schemas; and
- repository/bootstrap foundations.

## Phase B — Telemetry (P005–P009)

- Codex telemetry discovery;
- session detection;
- JSONL and incremental parsing; and
- token extraction.

## Phase C — Quota measurement (P010–P013)

- quota acquisition;
- reset handling;
- reconciliation; and
- observation correctness.

## Phase D — Storage (P014–P016)

- SQLite storage;
- migrations; and
- persisted observation model.

## Phase E — Estimator (P017–P020)

- Python analytics;
- robust statistics;
- outlier handling; and
- confidence-aware estimates.

## Phase F — History and prediction (P021–P023)

- historical estimates;
- remaining-workload prediction; and
- metering/quota-change detection.

## Phase G — Benchmarks (P024–P026)

- controlled benchmark framework;
- calibration experiments; and
- comparison/reporting.

## Phase H — Integration and hardening (P027–P030)

- CLI integration;
- privacy verification;
- cross-platform hardening; and
- release readiness.

Each phase SHOULD validate unknown assumptions rather than present undocumented OpenAI behavior as fact. Later implementation MUST continue to distinguish observed raw-token telemetry, Codex Meter-derived weighted/effective usage, and observed plan quota.
