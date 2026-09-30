# Codex Meter Product Contract

## Authority and normative language

This document is the authoritative product-level contract for Codex Meter. Future work MUST preserve it unless a later, explicit product decision supersedes it; such a change MUST be documented rather than introduced silently. **MUST**, **MUST NOT**, **SHOULD**, and **MAY** express requirement strength.

Codex Meter observes locally available evidence and produces empirical estimates. It does not claim knowledge of undocumented OpenAI internals. Unknown telemetry formats, meter behavior, and weighting rules remain subject to validation during later discovery work.

## Identity

| Property | Contract |
| --- | --- |
| Repository | `Codex-Meter` |
| Binary | `codex-meter` |
| Product | Codex Meter |
| Product status | Independent and unofficial |
| Primary initial plan | ChatGPT Plus |
| Primary v1 interface | CLI |
| Product model | Local-first and privacy-first |

Codex Meter tracks ChatGPT Codex / Work usage, measures local token telemetry, observes 5-hour and weekly quota consumption, empirically estimates token-equivalent quota capacity, compares historical measurements, supports controlled usage benchmarks, and estimates likely remaining workload. Its fundamental loop is:

```text
Track → Measure → Estimate → Compare
```

ChatGPT Plus is the initial priority, but the architecture SHOULD allow other plans without weakening dataset isolation.

## Core questions

Codex Meter should eventually answer:

- What is the estimated raw-token-equivalent capacity of the current 5-hour quota window?
- What is the estimated raw-token-equivalent capacity of the weekly quota?
- Approximately how many raw tokens have been consumed, and approximately how many remain?
- How many raw tokens correspond empirically to one quota percentage point?
- How stable is that estimate?
- Has the apparent metering regime changed historically?
- What does a typical light, normal, or heavy task consume?
- How do model, reasoning level, speed mode, caching, and task type correlate with quota burn?
- Approximately how many similar tasks could still fit in the remaining quota?

Answers MUST be presented as empirical estimates with appropriate uncertainty, not as facts about undocumented OpenAI mechanisms or allowances. Use language such as **Estimated 5-hour raw-token capacity**. The product MUST NOT say or imply **Your Plus plan has X tokens** unless OpenAI has explicitly documented that fact and the product clearly cites and distinguishes it from an estimate.

## Measurement domains

Codex Meter separates three measurement domains:

### Raw tokens

Raw tokens are direct token telemetry available from a local source. The conceptual model MUST support:

- uncached input;
- cached input;
- output;
- reasoning output, when available; and
- raw total.

A telemetry source MAY omit any field. Missing MUST remain distinguishable from zero, and a raw total MUST NOT be fabricated from unavailable components without labeling the result and its derivation.

### Weighted or effective usage

Weighted/effective usage is an optional Codex Meter-derived or normalized metric. A future derivation MAY account for uncached input, cached input, output, reasoning, model, and speed mode. Its inputs, formula/version, and uncertainty SHOULD be identifiable.

Weighted/effective usage MUST NOT be represented as an official OpenAI token allowance or as directly observed telemetry when it is derived.

### Plan quota

Plan quota is observed account-meter evidence, particularly:

- 5-hour used and/or remaining percentage;
- weekly used and/or remaining percentage; and
- reset timestamps or equivalent window identity metadata.

Whether a source reports used or remaining percentages MUST be explicit. Conversion between them is a calculation, not a new observation.

The invariant is:

```text
raw tokens != weighted/effective usage != plan quota
```

No implementation may silently equate, substitute, or pool these domains. A relationship between them is an empirical inference and MUST retain its provenance and uncertainty.

## Observation semantics

An `Observation` is the conceptual unit of measurable activity associated with a Codex task or session. It is not a database schema. Its future representation should be capable of carrying the following, with fields absent where evidence is unavailable.

### Environment and configuration

- plan;
- model;
- reasoning level;
- speed mode;
- Codex version; and
- relevant timestamps.

### Task or session information

- start time;
- end time; and
- duration.

### Token telemetry

- uncached input;
- cached input;
- output;
- reasoning output, if available; and
- raw total.

### Quota measurements

For each of the 5-hour and weekly windows, when available:

- quota before;
- quota after;
- quota delta;
- reset/window identity, reset timestamps, or equivalent reset metadata; and
- evidence timing and finalization state sufficient for reconciliation.

### Derived measurements

An observation MAY support derived values such as:

- raw tokens per quota percentage point;
- tokens per minute; and
- quota percentage points per minute.

Derived values MUST be labeled as derived and MUST NOT conceal missing inputs, reset crossings, or unresolved meter state. Concrete persistence and schema design belong to later prompts.

## Observation quality

The stable quality semantics are:

| Grade | Meaning | Suggested estimator treatment |
| --- | --- | --- |
| `A` | Controlled benchmark | Weight 1.0 |
| `B` | Isolated normal task | Weight 0.8 |
| `C` | Possible concurrent Codex/Work consumption | Weight 0.3 |
| `D` | Delayed or incomplete meter evidence | Ignore by default |
| `X` | Invalid for estimation | Reject |

The grade communicates evidence quality and MUST be retained independently of estimator policy. The listed weights are initial suggestions, not permanent constants. Exact weighting is an estimator implementation detail and MAY evolve, while the semantic distinctions MUST remain stable or be explicitly versioned and migrated.

## Meter reconciliation is a correctness requirement

Quota meters may update after local token telemetry. A task ending MUST NOT automatically imply that its quota observation is final. The conceptual sequence is:

```text
task ends
↓
capture local token telemetry
↓
sample quota meter
↓
if unchanged, delayed, or unstable:
    retry after short intervals
↓
wait for meter stabilization
↓
finalize observation
```

Samples at T+0, T+15s, T+30s, and T+60s illustrate a possible policy only. Discovery and implementation evidence MUST determine configurable retry, timeout, and stabilization rules; these example timings are not permanent requirements.

An implementation MUST preserve provisional state and evidence quality when stabilization cannot be established. Meter reconciliation is a correctness feature, not cosmetic polish.

## Reset behavior is a correctness requirement

A reset crossing invalidates the affected window's contribution to capacity estimation. For example, if a used-percentage meter reports:

```text
before = 97%
after  = 4%
```

Codex Meter MUST NOT calculate:

```text
delta = -93 percentage points
```

It should instead record the conceptual result:

```text
5-hour estimate contribution: invalid
reason: quota window reset during measurement
```

The same rule applies independently to weekly resets. Reset metadata or defensible window identity evidence SHOULD drive detection. A reset in one window need not invalidate another window with demonstrably continuous identity. Token telemetry MAY remain valid for task-size, throughput, and other statistics that do not depend on the crossed quota window.

## Estimator principles

The estimator MUST NOT infer full quota capacity from one observation. Future implementations MUST use multiple compatible observations and robust, confidence-aware reporting. They MUST consider:

- median;
- weighted median where appropriate;
- MAD-based or comparably robust outlier handling;
- P25, P50, and P75;
- sample count;
- stability or variance information;
- a confidence classification or interval; and
- observation-quality weighting.

Reports SHOULD expose sample sufficiency and instability rather than manufacture precision. Statistical methods MAY evolve as evidence accumulates, provided they preserve robust estimation, confidence awareness, reproducibility/versioning, and the separation of observed inputs from inferred results.

## Dataset isolation

Incompatible measurements MUST NOT be silently pooled. Estimator grouping or isolation MUST account at minimum for:

- plan;
- model;
- reasoning level;
- speed mode; and
- relevant time period or metering regime.

Codex version MAY become a grouping or segmentation dimension when evidence shows it matters. Missing configuration MUST NOT be assumed equivalent to a known value. For example:

```text
plus / gpt-5.6-sol / high / standard
```

must not automatically be combined with:

```text
plus / gpt-5.6-sol / high / fast
```

or with another model family. Cross-group comparisons MAY be shown, but MUST remain labeled rather than becoming a pooled estimate by accident.

## Privacy contract

Codex Meter is local-first and MUST collect only the minimum metadata needed for measurement and estimation. Normal telemetry MUST NOT store:

- prompts;
- model responses;
- source code;
- repository contents;
- Git remotes;
- user email;
- credentials;
- OAuth tokens; or
- access tokens.

Parsers and diagnostic paths MUST avoid copying prohibited content merely because it coexists with allowed numerical telemetry. Logs, errors, exports, fixtures, and backups SHOULD follow the same minimization rule.

A future community dataset MAY accept sanitized numerical observations only. Upload MUST always require explicit opt-in, SHOULD preview the exact exported data, and MUST NOT be enabled implicitly. Community upload is outside P001 and v1 implementation work in this prompt.

## Major product components

These are product concepts, not commitments to final modules, packages, processes, or crate boundaries:

- **Tracker:** acquires local telemetry and quota observations, including evidence needed for reset detection and reconciliation.
- **Estimator:** produces robust empirical quota-capacity estimates from compatible, quality-scored observations.
- **Task history:** retains local historical observations and estimates with sufficient provenance for comparison.
- **Benchmark system:** runs controlled experiments primarily to improve estimator quality and calibration.
- **Remaining-workload estimator:** maps remaining estimated capacity onto distributions of prior comparable task sizes.
- **Community dataset:** a future, optional facility for explicitly opted-in, sanitized numerical observations.

## Implementation-language responsibilities

### Rust: measurement and runtime layer

Rust is intended to own:

- the long-running local agent;
- filesystem and session watching;
- incremental JSONL parsing;
- Codex session detection;
- quota sampling;
- reset detection;
- meter reconciliation;
- realtime statistics;
- SQLite writing;
- the CLI/runtime core; and
- cross-platform native behavior.

### Python: analytics and research layer

Python is intended to own:

- the estimator;
- robust statistics;
- regression;
- outlier analysis;
- confidence intervals;
- benchmark analysis;
- historical analysis;
- quota-change detection; and
- dataset/community research.

### Rust–Python boundary

The initial boundary is:

```text
SQLite
+
versioned JSON schemas
```

Boundary formats MUST be versioned and preserve observation provenance and missingness. P001 introduces neither a SQLite schema nor JSON schemas. PyO3, maturin, or another FFI bridge MUST NOT be introduced at this stage; they MAY be evaluated later only for a demonstrated concrete need.

## v1 scope and non-goals

v1 SHOULD remain local-first, CLI-first, automatic, privacy-preserving, Plus-first, empirical, history-aware, confidence-aware, and capable of controlled benchmarks.

Explicit v1 non-goals are:

- a general OpenAI/API billing platform;
- a prompt logger;
- a response logger;
- a repository analyzer;
- an AI observability SaaS;
- an OpenAI credential manager;
- a web dashboard; and
- an arbitrary general-purpose AI benchmark platform.

No TypeScript/React web UI or C/C++ is required for v1. Docker is not a runtime requirement because Codex Meter needs access to local Codex/account telemetry; packaging research MAY revisit deployment options without making containers necessary for ordinary operation.

P001 specifically does not implement watchers, parsers, quota clients, storage schemas, estimators, benchmark execution, a TUI, a web dashboard, or community upload.

## Expected CLI surface

The intended high-level interface is:

```text
codex-meter
codex-meter status
codex-meter live
codex-meter estimate
codex-meter tasks
codex-meter task <id>
codex-meter history
codex-meter benchmark
```

Potential later commands are:

```text
codex-meter can-run
codex-meter export --anonymous
codex-meter community
```

Running `codex-meter` should likely behave similarly to `codex-meter status`. This is a product UX contract and command-planning input, not a P001 implementation requirement. Exact flags and output schemas remain subject to later design, but command behavior MUST preserve the measurement-domain, uncertainty, and privacy contracts above.
