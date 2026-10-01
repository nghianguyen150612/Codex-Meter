# Prompt 003A Manifest

## Identification and baseline

- **Prompt:** P003A — Tighten v1 Data Contract Invariants
- **Starting SHA:** `d8b10c2b95bf30d77cfa2df12cc5832c1e2b8750`
- **Branch:** `work`
- **P003 baseline:** The checkout contains the P003 content as commit `d8b10c2` (`Define Codex Meter v1 data contracts`). The prompt-named SHA `c8fce48701c7e96a27eb992bb4d5ba73aa782965` is not an object in this checkout; no reset or history rewrite was performed. The worktree was clean.
- **Final SHA:** Not embedded because a commit cannot contain its own SHA; the completion report records it.

## Defects corrected

- The `five_hour` and `weekly` observation branches now require their corresponding meter-type constants.
- An available observation window identity must use the meter type of its containing quota branch.
- Normalized event types now conditionally require the matching lifecycle, token-counter, or configuration payload discriminator.
- An available quota-sample reset/window identity must use the sample's top-level meter type.
- Existing reset/delta exclusion, independent evidence validity, privacy, strictness, units, and provider-independent window semantics remain unchanged.

## Files modified

- `schemas/v1/observation.schema.json`
- `schemas/v1/normalized-event.schema.json`
- `schemas/v1/quota-sample.schema.json`
- `docs/DATA_CONTRACTS.md`

## Files created and negative fixtures

- `docs/PROMPT003A_MANIFEST.md`
- `fixtures/contracts/v1/invalid/observation-five-hour-as-weekly.json`
- `fixtures/contracts/v1/invalid/observation-weekly-as-five-hour.json`
- `fixtures/contracts/v1/invalid/event-session-started-token-payload.json`
- `fixtures/contracts/v1/invalid/event-token-update-lifecycle-payload.json`
- `fixtures/contracts/v1/invalid/quota-sample-reset-meter-mismatch.json`

Files under `fixtures/contracts/v1/invalid/` are deliberately contradictory negative-validation examples and must fail validation. All data is synthetic.

## Schema version decision

The schemas remain at `1.0.0`. These constraints correct machine validation to enforce P003's already documented semantics; they neither add a domain concept nor redefine accepted meaning. This is a validation correction under the documented patch policy, with no need to change the serialized version constant.

## Validation and scope

Validation parses all JSON, resolves local references, checks targeted `const` and conditional structures, verifies normal fixture consistency, and proves each negative fixture represents the intended rejected combination. If no Draft 2020-12 validator is installed, fixture-to-schema execution remains deferred to P004 and is reported explicitly. P003A adds no runtime source, dependency/tooling, SQL, CI, discovery, acquisition, reconciliation, estimator, benchmark, or UI work. **P004 was not started.**

## Unrelated work

No unrelated work was modified. This corrective change is limited to the named schemas, minimal semantic documentation, negative fixtures, and this manifest.
