# ADR-001: Runtime and analytics boundary

## Status

Accepted

## Context

Codex Meter needs a reliable local runtime for discovery, incremental measurement, quota reconciliation, persistence, and a CLI, while its empirical estimator needs research-oriented robust statistics. The boundary must preserve privacy, permit independent testing, avoid competing database writers, and tolerate analytics failure without losing observations. The product is early enough that source formats and quota mechanisms remain unvalidated.

## Decision

1. Rust is the primary runtime, measurement, benchmark-execution, and CLI layer.
2. Python is the analytics and statistics layer, not the telemetry collector or long-running tracker.
3. SQLite is the durable local observation/history boundary.
4. Rust owns database creation, migrations, primary mutation, and transactional runtime updates.
5. Python normally consumes an explicitly allowed SQLite view/data set read-only.
6. Rust invokes Python at a process boundary; requests and results use versioned JSON contracts. Rust validates results and owns any durable result write.
7. PyO3/maturin or other FFI is intentionally deferred unless later evidence demonstrates a concrete need.
8. We favor testability, failure isolation, explicit contracts, and loose coupling over the lowest possible per-call overhead.

Exact process/package invocation and schemas are deferred. Rust and Python must be independently testable, and Python analytics should be independently runnable for research.

## Consequences

- There is one migration authority and primary SQLite writer, reducing lock and schema ambiguity.
- Analytics can evolve without embedding a Python interpreter in the tracker.
- Version negotiation and validation are explicit; malformed output cannot directly mutate storage.
- Python startup and JSON serialization add overhead, accepted because estimation is not expected to require an in-process hot path.
- Packaging must eventually arrange a compatible Python environment and provide explicit errors when it is unavailable.
- Tracking can continue and evidence remains durable when analytics fails; estimation failure stays distinct from tracking failure.
- Derived results that require persistence make a validated round trip through Rust.

## Alternatives considered

### Rust-only implementation

This would simplify distribution and eliminate the language boundary, but is not selected initially because Python better supports rapid statistical research and mature analysis workflows. It may be reconsidered for proven, stable hot paths.

### Python-only implementation

This would remove cross-language contracts, but is not selected because the long-running, platform-aware tracker, transactional runtime, and primary CLI benefit from Rust's deployment and runtime characteristics. It would also blur collection and analytics ownership.

### PyO3/maturin embedding

Embedding could reduce process-call overhead and offer typed calls, but it couples build, packaging, interpreter lifecycle, and failure modes too early. No demonstrated performance need currently outweighs that complexity. It remains reconsiderable.

### Both languages writing SQLite freely

Multiple writers could make result persistence direct, but introduce competing migration authority, locking behavior, and transaction semantics. Read-only Python plus validated Rust persistence is clearer for v1.

### HTTP or local service boundary

A service API could isolate processes and support richer interactions, but adds ports, lifecycle, authentication/security, versioning, and daemon complexity without a v1 need. A service boundary may be reconsidered if future requirements justify it.
