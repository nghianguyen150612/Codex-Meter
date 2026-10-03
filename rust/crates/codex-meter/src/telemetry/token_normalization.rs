//! Raw token extraction and double-count-safe normalization.

use std::fmt;

use super::codex_rollout::{
    EventMessage, RolloutRecord, RolloutRecordKind, TokenUsage, TokenUsageRecord,
};
use super::identity;
use super::incremental_jsonl::{ReadItem, ReadItemOutcome, RejectedLineReason, SourceIdentity};
use super::normalized::{
    NormalizedEventType, NormalizedTokenEvent, NormalizedTokenPayload, SchemaVersion,
    TokenCounters, TokenPayloadKind,
};

/// The semantic role of one source token-usage structure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TokenEvidenceSemantic {
    PerResponse,
    TurnCumulativeSnapshot,
    ThreadCumulativeSnapshot,
    TokenCountTotalSnapshot,
    TokenCountLatestSnapshot,
}

/// Raw source counters plus their non-interchangeable semantic role.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RawTokenEvidence {
    pub semantic: TokenEvidenceSemantic,
    pub usage: TokenUsage,
}

/// Named evidence extracted from one token-bearing source record.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TokenEvidenceSet {
    pub per_response: Option<RawTokenEvidence>,
    pub turn_cumulative: Option<RawTokenEvidence>,
    pub thread_cumulative: Option<RawTokenEvidence>,
    pub total_snapshot: Option<RawTokenEvidence>,
    pub latest_snapshot: Option<RawTokenEvidence>,
}

/// Token-bearing classification before normalization.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TokenEvidenceExtraction {
    PerResponse {
        evidence: TokenEvidenceSet,
        session_id: String,
        turn_id: String,
    },
    TokenCountSnapshot {
        evidence: TokenEvidenceSet,
    },
    NotTokenBearing,
}

/// Result of normalizing one P007 item.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TokenNormalizationOutcome {
    PerResponse {
        raw_evidence: Box<TokenEvidenceSet>,
        normalized_event: Box<NormalizedTokenEvent>,
    },
    TokenCountSnapshot {
        raw_evidence: Box<TokenEvidenceSet>,
    },
    NotTokenBearing,
    RejectedSourceLine {
        reason: RejectedLineReason,
    },
}

/// Structural errors at the raw-token normalization boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TokenNormalizationError {
    InvalidTimestamp(TimestampErrorReason),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TimestampErrorReason {
    WrongShape,
    InvalidDate,
    InvalidTime,
}

impl fmt::Display for TokenNormalizationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidTimestamp(reason) => {
                write!(
                    formatter,
                    "source timestamp is not valid UTC RFC 3339: {reason}"
                )
            }
        }
    }
}

impl fmt::Display for TimestampErrorReason {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::WrongShape => "wrong shape",
            Self::InvalidDate => "invalid date",
            Self::InvalidTime => "invalid time",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for TokenNormalizationError {}

/// Extracts named raw evidence without creating normalized consumption.
pub fn extract_token_evidence(record: &RolloutRecord) -> TokenEvidenceExtraction {
    match &record.kind {
        RolloutRecordKind::TokenUsageRecord(usage) => TokenEvidenceExtraction::PerResponse {
            evidence: usage_evidence(usage),
            session_id: usage.session_id.clone(),
            turn_id: usage.turn_id.clone(),
        },
        RolloutRecordKind::EventMsg(EventMessage::TokenCount(event)) => {
            let mut evidence = TokenEvidenceSet::default();
            if let Some(info) = &event.info {
                evidence.total_snapshot = Some(RawTokenEvidence {
                    semantic: TokenEvidenceSemantic::TokenCountTotalSnapshot,
                    usage: info.total_token_usage.clone(),
                });
                evidence.latest_snapshot = Some(RawTokenEvidence {
                    semantic: TokenEvidenceSemantic::TokenCountLatestSnapshot,
                    usage: info.last_token_usage.clone(),
                });
            }
            TokenEvidenceExtraction::TokenCountSnapshot { evidence }
        }
        _ => TokenEvidenceExtraction::NotTokenBearing,
    }
}

/// Normalizes only canonical per-response usage from one complete P007 item.
pub fn normalize_token_item(
    source: &SourceIdentity,
    item: &ReadItem,
) -> Result<TokenNormalizationOutcome, TokenNormalizationError> {
    let record = match &item.outcome {
        ReadItemOutcome::Decoded(record) => record.as_ref(),
        ReadItemOutcome::Rejected(rejected) => {
            return Ok(TokenNormalizationOutcome::RejectedSourceLine {
                reason: rejected.reason.clone(),
            });
        }
    };

    match extract_token_evidence(record) {
        TokenEvidenceExtraction::PerResponse {
            evidence,
            session_id,
            turn_id,
        } => {
            validate_utc_timestamp(&record.timestamp)?;
            let per_response_usage = &evidence
                .per_response
                .as_ref()
                .expect("per-response extraction includes canonical usage")
                .usage;
            let normalized_event = NormalizedTokenEvent {
                schema_version: SchemaVersion::V1,
                event_id: identity::token_event_id(
                    source,
                    item.start_offset,
                    item.end_offset,
                    item.ordinal,
                ),
                event_type: NormalizedEventType::TokenCountersUpdated,
                source_instance_id: identity::source_instance_id(source),
                safe_cursor_id: Some(identity::cursor_id(source, item.end_offset)),
                event_at: record.timestamp.clone(),
                session_id: Some(identity::session_id(&session_id)),
                task_id: Some(identity::task_id(&turn_id)),
                payload: NormalizedTokenPayload {
                    kind: TokenPayloadKind::TokenCounters,
                    token_counters: TokenCounters::from_per_response(per_response_usage),
                },
            };
            Ok(TokenNormalizationOutcome::PerResponse {
                raw_evidence: Box::new(evidence),
                normalized_event: Box::new(normalized_event),
            })
        }
        TokenEvidenceExtraction::TokenCountSnapshot { evidence } => {
            Ok(TokenNormalizationOutcome::TokenCountSnapshot {
                raw_evidence: Box::new(evidence),
            })
        }
        TokenEvidenceExtraction::NotTokenBearing => Ok(TokenNormalizationOutcome::NotTokenBearing),
    }
}

fn usage_evidence(usage: &TokenUsageRecord) -> TokenEvidenceSet {
    TokenEvidenceSet {
        per_response: Some(RawTokenEvidence {
            semantic: TokenEvidenceSemantic::PerResponse,
            usage: usage.usage.clone(),
        }),
        turn_cumulative: Some(RawTokenEvidence {
            semantic: TokenEvidenceSemantic::TurnCumulativeSnapshot,
            usage: usage.turn_token_usage.clone(),
        }),
        thread_cumulative: Some(RawTokenEvidence {
            semantic: TokenEvidenceSemantic::ThreadCumulativeSnapshot,
            usage: usage.thread_token_usage.clone(),
        }),
        ..TokenEvidenceSet::default()
    }
}

pub(crate) fn validate_utc_timestamp(value: &str) -> Result<(), TokenNormalizationError> {
    validate_timestamp(value).map_err(TokenNormalizationError::InvalidTimestamp)
}

pub(crate) fn validate_timestamp(value: &str) -> Result<(), TimestampErrorReason> {
    let bytes = value.as_bytes();
    if bytes.len() < 20 || bytes[4] != b'-' || bytes[7] != b'-' || bytes[10] != b'T' {
        return Err(TimestampErrorReason::WrongShape);
    }
    if bytes[13] != b':' || bytes[16] != b':' {
        return Err(TimestampErrorReason::WrongShape);
    }
    let fraction_end = bytes.len() - 1;
    if bytes[fraction_end] != b'Z' {
        return Err(TimestampErrorReason::WrongShape);
    }
    if fraction_end > 19
        && (bytes[19] != b'.'
            || fraction_end == 20
            || !bytes[20..fraction_end].iter().all(u8::is_ascii_digit))
    {
        return Err(TimestampErrorReason::WrongShape);
    }
    let year = decimal(&bytes[0..4]);
    let month = decimal(&bytes[5..7]);
    let day = decimal(&bytes[8..10]);
    let hour = decimal(&bytes[11..13]);
    let minute = decimal(&bytes[14..16]);
    let second = decimal(&bytes[17..19]);
    if [year, month, day, hour, minute, second]
        .iter()
        .any(Option::is_none)
    {
        return Err(TimestampErrorReason::WrongShape);
    }
    let year = year.expect("checked above");
    let month = month.expect("checked above");
    let day = day.expect("checked above");
    let hour = hour.expect("checked above");
    let minute = minute.expect("checked above");
    let second = second.expect("checked above");
    if month == 0 || month > 12 || day == 0 || day > days_in_month(year, month) {
        return Err(TimestampErrorReason::InvalidDate);
    }
    if hour > 23 || minute > 59 || second > 59 {
        return Err(TimestampErrorReason::InvalidTime);
    }
    Ok(())
}

fn decimal(bytes: &[u8]) -> Option<u32> {
    bytes.iter().try_fold(0_u32, |value, byte| {
        byte.is_ascii_digit()
            .then_some(value * 10 + u32::from(byte - b'0'))
    })
}

fn days_in_month(year: u32, month: u32) -> u32 {
    match month {
        2 if year.is_multiple_of(400) || (year.is_multiple_of(4) && !year.is_multiple_of(100)) => {
            29
        }
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{json, Value};

    use super::*;
    use crate::telemetry::codex_rollout::parse_rollout_record;
    use crate::telemetry::incremental_jsonl::ReadItem;
    use crate::telemetry::normalized::{MetricAvailability, MetricProvenance};

    const TOKEN_USAGE_FIXTURE: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../fixtures/codex-rollout/v0.157.1/token-usage-record.json"
    ));
    const TOKEN_USAGE_MISSING_REASONING_FIXTURE: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../fixtures/codex-rollout/v0.157.1/token-usage-missing-reasoning.json"
    ));
    const TOKEN_USAGE_ZERO_REASONING_FIXTURE: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../fixtures/codex-rollout/v0.157.1/token-usage-zero-reasoning.json"
    ));
    const TOKEN_COUNT_FIXTURE: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../fixtures/codex-rollout/v0.157.1/token-count.json"
    ));
    const CODEX_NORMALIZED_FIXTURE: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../fixtures/contracts/v1/normalized-token-event-codex-rollout.json"
    ));

    fn source(generation: &str) -> SourceIdentity {
        SourceIdentity::new("rollout-normalization-synthetic", generation)
            .expect("synthetic source identity should be valid")
    }

    fn usage_item(input: &str, start_offset: u64, end_offset: u64) -> ReadItem {
        let record = parse_rollout_record(input).expect("synthetic token record should decode");
        let ordinal = record.ordinal;
        ReadItem {
            start_offset,
            end_offset,
            ordinal,
            outcome: ReadItemOutcome::Decoded(Box::new(record)),
        }
    }

    fn usage_item_from_json(value: Value, start_offset: u64) -> ReadItem {
        let input = value.to_string();
        let end_offset = start_offset + input.len() as u64 + 1;
        usage_item(&input, start_offset, end_offset)
    }

    fn token_usage_payload(
        usage: Value,
        turn_token_usage: Value,
        thread_token_usage: Value,
    ) -> Value {
        json!({
            "timestamp": "2026-10-03T10:01:00Z",
            "ordinal": 20,
            "type": "token_usage_record",
            "payload": {
                "thread_id": "thread-normalization-synthetic",
                "turn_id": "turn-normalization-synthetic",
                "session_id": "session-normalization-synthetic",
                "usage": usage,
                "turn_token_usage": turn_token_usage,
                "thread_token_usage": thread_token_usage
            }
        })
    }

    fn complete_usage() -> Value {
        json!({
            "input_tokens": 1000,
            "cached_input_tokens": 250,
            "cache_write_input_tokens": 17,
            "output_tokens": 4,
            "reasoning_output_tokens": 5,
            "total_tokens": 999
        })
    }

    #[test]
    fn extracts_named_per_response_and_cumulative_evidence() {
        let record = parse_rollout_record(TOKEN_USAGE_FIXTURE).expect("fixture should decode");
        let TokenEvidenceExtraction::PerResponse { evidence, .. } = extract_token_evidence(&record)
        else {
            panic!("expected per-response extraction");
        };

        assert_eq!(
            evidence
                .per_response
                .as_ref()
                .expect("per-response")
                .semantic,
            TokenEvidenceSemantic::PerResponse
        );
        assert_eq!(
            evidence
                .turn_cumulative
                .as_ref()
                .expect("turn snapshot")
                .semantic,
            TokenEvidenceSemantic::TurnCumulativeSnapshot
        );
        assert_eq!(
            evidence
                .thread_cumulative
                .as_ref()
                .expect("thread snapshot")
                .semantic,
            TokenEvidenceSemantic::ThreadCumulativeSnapshot
        );
        assert_eq!(
            evidence
                .per_response
                .as_ref()
                .expect("per-response")
                .usage
                .total_tokens
                .expect("total")
                .get(),
            100
        );
        assert_eq!(
            evidence
                .turn_cumulative
                .as_ref()
                .expect("turn snapshot")
                .usage
                .total_tokens
                .expect("total")
                .get(),
            300
        );
        assert_eq!(
            evidence
                .thread_cumulative
                .as_ref()
                .expect("thread snapshot")
                .usage
                .total_tokens
                .expect("total")
                .get(),
            900
        );
    }

    #[test]
    fn maps_only_per_response_usage_to_normalized_counters() {
        let source = source("generation-1");
        let item = usage_item(TOKEN_USAGE_FIXTURE, 10, 100);
        let TokenNormalizationOutcome::PerResponse {
            raw_evidence,
            normalized_event,
        } = normalize_token_item(&source, &item).expect("normalization should succeed")
        else {
            panic!("expected normalized per-response event");
        };

        let counters = normalized_event.payload.token_counters;
        assert_eq!(
            counters.uncached_input.availability,
            MetricAvailability::Unavailable
        );
        assert_eq!(counters.uncached_input.value, None);
        assert_eq!(counters.cached_input.value, Some(20));
        assert_eq!(counters.output.value, Some(30));
        assert_eq!(counters.reasoning_output.value, Some(10));
        assert_eq!(counters.raw_total.value, Some(100));
        assert_eq!(counters.raw_total.provenance, MetricProvenance::Observed);
        assert_eq!(
            raw_evidence
                .per_response
                .expect("per-response evidence")
                .usage
                .cache_write_input_tokens
                .expect("cache-write evidence")
                .get(),
            5
        );
    }

    #[test]
    fn cumulative_snapshots_do_not_change_canonical_raw_total() {
        let source = source("generation-1");
        let item = usage_item(TOKEN_USAGE_FIXTURE, 10, 100);
        let TokenNormalizationOutcome::PerResponse {
            raw_evidence,
            normalized_event,
        } = normalize_token_item(&source, &item).expect("normalization should succeed")
        else {
            panic!("expected normalized per-response event");
        };

        assert_eq!(
            normalized_event.payload.token_counters.raw_total.value,
            Some(100)
        );
        assert_eq!(
            raw_evidence
                .turn_cumulative
                .expect("turn snapshot")
                .usage
                .total_tokens
                .expect("turn total")
                .get(),
            300
        );
        assert_eq!(
            raw_evidence
                .thread_cumulative
                .expect("thread snapshot")
                .usage
                .total_tokens
                .expect("thread total")
                .get(),
            900
        );
    }

    #[test]
    fn token_count_snapshots_are_evidence_without_consumption_events() {
        let source = source("generation-1");
        let item = usage_item(TOKEN_COUNT_FIXTURE, 0, 500);
        let TokenNormalizationOutcome::TokenCountSnapshot { raw_evidence } =
            normalize_token_item(&source, &item).expect("snapshot normalization should succeed")
        else {
            panic!("expected snapshot evidence");
        };

        assert!(raw_evidence.per_response.is_none());
        assert_eq!(
            raw_evidence
                .total_snapshot
                .expect("total snapshot")
                .semantic,
            TokenEvidenceSemantic::TokenCountTotalSnapshot
        );
        assert_eq!(
            raw_evidence
                .latest_snapshot
                .expect("latest snapshot")
                .usage
                .total_tokens
                .expect("latest total")
                .get(),
            100
        );
    }

    #[test]
    fn missing_and_zero_reasoning_values_remain_distinct() {
        let source = source("generation-1");
        let missing = usage_item(TOKEN_USAGE_MISSING_REASONING_FIXTURE, 0, 1);
        let zero = usage_item(TOKEN_USAGE_ZERO_REASONING_FIXTURE, 2, 3);
        let TokenNormalizationOutcome::PerResponse {
            normalized_event: missing_event,
            ..
        } = normalize_token_item(&source, &missing).expect("missing normalization")
        else {
            panic!("expected missing event");
        };
        let TokenNormalizationOutcome::PerResponse {
            normalized_event: zero_event,
            ..
        } = normalize_token_item(&source, &zero).expect("zero normalization")
        else {
            panic!("expected zero event");
        };

        assert_eq!(
            missing_event
                .payload
                .token_counters
                .reasoning_output
                .availability,
            MetricAvailability::Unavailable
        );
        assert_eq!(
            zero_event
                .payload
                .token_counters
                .reasoning_output
                .availability,
            MetricAvailability::Available
        );
        assert_eq!(
            zero_event.payload.token_counters.reasoning_output.value,
            Some(0)
        );
    }

    #[test]
    fn input_and_cache_write_are_not_relabelled_or_reassigned() {
        let source = source("generation-1");
        let usage = complete_usage();
        let item =
            usage_item_from_json(token_usage_payload(usage.clone(), usage.clone(), usage), 10);
        let TokenNormalizationOutcome::PerResponse {
            raw_evidence,
            normalized_event,
        } = normalize_token_item(&source, &item).expect("normalization should succeed")
        else {
            panic!("expected normalized event");
        };
        let counters = normalized_event.payload.token_counters;

        assert_eq!(
            counters.uncached_input.availability,
            MetricAvailability::Unavailable
        );
        assert_eq!(counters.cached_input.value, Some(250));
        assert_eq!(counters.output.value, Some(4));
        assert_eq!(counters.reasoning_output.value, Some(5));
        assert_eq!(counters.raw_total.value, Some(999));
        assert_eq!(counters.raw_total.provenance, MetricProvenance::Observed);
        let raw = raw_evidence
            .per_response
            .expect("raw per-response evidence")
            .usage;
        assert_eq!(raw.input_tokens.expect("input").get(), 1000);
        assert_eq!(raw.cache_write_input_tokens.expect("cache write").get(), 17);
    }

    #[test]
    fn missing_total_remains_unavailable() {
        let source = source("generation-1");
        let usage = json!({
            "input_tokens": 1000,
            "cached_input_tokens": 250,
            "cache_write_input_tokens": 17,
            "output_tokens": 4,
            "reasoning_output_tokens": 5
        });
        let item =
            usage_item_from_json(token_usage_payload(usage.clone(), usage.clone(), usage), 10);
        let TokenNormalizationOutcome::PerResponse {
            normalized_event, ..
        } = normalize_token_item(&source, &item).expect("normalization should succeed")
        else {
            panic!("expected normalized event");
        };
        assert_eq!(
            normalized_event
                .payload
                .token_counters
                .raw_total
                .availability,
            MetricAvailability::Unavailable
        );
        assert_eq!(
            normalized_event.payload.token_counters.raw_total.value,
            None
        );
    }

    #[test]
    fn replay_produces_identical_event_and_opaque_ids() {
        let source = source("generation-1");
        let item = usage_item(TOKEN_USAGE_FIXTURE, 100, 200);
        let first = normalize_token_item(&source, &item).expect("first normalization");
        let second = normalize_token_item(&source, &item).expect("replay normalization");
        assert_eq!(first, second);

        let TokenNormalizationOutcome::PerResponse {
            normalized_event, ..
        } = first
        else {
            panic!("expected per-response event");
        };
        assert!(normalized_event.event_id.starts_with("evt:"));
        assert!(normalized_event.source_instance_id.starts_with("src:"));
        assert!(normalized_event
            .safe_cursor_id
            .as_deref()
            .expect("cursor id")
            .starts_with("cursor:"));
        assert!(normalized_event
            .session_id
            .as_deref()
            .expect("session id")
            .starts_with("session:"));
        assert!(normalized_event
            .task_id
            .as_deref()
            .expect("task id")
            .starts_with("task:"));
        let serialized = serde_json::to_string(&normalized_event).expect("event should serialize");
        for sensitive_value in [
            "rollout-normalization-synthetic",
            "generation-1",
            "session-synthetic-001",
            "turn-synthetic-001",
        ] {
            assert!(!serialized.contains(sensitive_value));
        }
        for identifier in [
            &normalized_event.event_id,
            &normalized_event.source_instance_id,
            normalized_event.safe_cursor_id.as_ref().expect("cursor id"),
            normalized_event.session_id.as_ref().expect("session id"),
            normalized_event.task_id.as_ref().expect("task id"),
        ] {
            assert!(identifier.len() <= 128);
            assert!(identifier
                .chars()
                .all(|character| character.is_ascii_alphanumeric()
                    || matches!(character, '.' | '_' | ':' | '-')));
        }
    }

    #[test]
    fn p008_identity_domains_remain_stable_after_shared_refactor() {
        let source = source("generation-1");
        let item = usage_item(TOKEN_USAGE_FIXTURE, 10, 100);
        let TokenNormalizationOutcome::PerResponse {
            normalized_event, ..
        } = normalize_token_item(&source, &item).expect("normalization should succeed")
        else {
            panic!("expected normalized event");
        };
        assert_eq!(
            normalized_event.source_instance_id,
            "src:6fc10234e90b5a972ceb75dc86da9ade29e1d1fa67bdc50644ff91eb7d25c2a3"
        );
        assert_eq!(
            normalized_event.event_id,
            "evt:e9fa822c657c9622b47ebd66302d9b525831013c69835bc0be605ea4abedcfb7"
        );
        assert_eq!(
            normalized_event.safe_cursor_id.as_deref(),
            Some("cursor:17575161849f536ed81dc3e411c6c88629849a3e34765a01bcde9d387a33301d")
        );
        assert_eq!(
            normalized_event.session_id.as_deref(),
            Some("session:ec34cebfd1edf04478a1f3403718816ddd84f723a3e97c22310dbc0faddf665d")
        );
        assert_eq!(
            normalized_event.task_id.as_deref(),
            Some("task:029bdc773ec84bb9e449cc050cccb13ec4652f187be878d15165994bc9e1f70f")
        );
    }

    #[test]
    fn position_and_generation_change_event_identity() {
        let source_one = source("generation-1");
        let source_two = source("generation-2");
        let first_item = usage_item(TOKEN_USAGE_FIXTURE, 100, 200);
        let second_item = usage_item(TOKEN_USAGE_FIXTURE, 201, 301);
        let first = normalized_event(&source_one, &first_item);
        let second = normalized_event(&source_one, &second_item);
        let replacement = normalized_event(&source_two, &first_item);

        assert_ne!(first.event_id, second.event_id);
        assert_ne!(first.safe_cursor_id, second.safe_cursor_id);
        assert_ne!(first.source_instance_id, replacement.source_instance_id);
        assert_ne!(first.event_id, replacement.event_id);
        assert_ne!(first.safe_cursor_id, replacement.safe_cursor_id);
    }

    #[test]
    fn non_token_and_rejected_items_do_not_create_zero_events() {
        let source = source("generation-1");
        let non_token = parse_rollout_record(
            r#"{"timestamp":"2026-10-03T10:00:00Z","type":"session_meta","payload":{"session_id":"s","id":"t","cli_version":"0.157.1"}}"#,
        )
        .expect("session metadata should decode");
        let non_token_item = ReadItem {
            start_offset: 0,
            end_offset: 1,
            ordinal: non_token.ordinal,
            outcome: ReadItemOutcome::Decoded(Box::new(non_token)),
        };
        assert_eq!(
            normalize_token_item(&source, &non_token_item).expect("non-token normalization"),
            TokenNormalizationOutcome::NotTokenBearing
        );

        let rejected_item = ReadItem {
            start_offset: 2,
            end_offset: 3,
            ordinal: None,
            outcome: ReadItemOutcome::Rejected(super::super::incremental_jsonl::RejectedLine {
                reason: RejectedLineReason::EmptyLine,
            }),
        };
        assert!(matches!(
            normalize_token_item(&source, &rejected_item).expect("rejected normalization"),
            TokenNormalizationOutcome::RejectedSourceLine {
                reason: RejectedLineReason::EmptyLine
            }
        ));
    }

    #[test]
    fn invalid_timestamp_is_rejected_without_exposing_source_value() {
        let source = source("generation-1");
        let mut record = parse_rollout_record(TOKEN_USAGE_FIXTURE).expect("fixture should decode");
        record.timestamp = "not-a-real-source-timestamp".to_owned();
        let item = ReadItem {
            start_offset: 0,
            end_offset: 1,
            ordinal: record.ordinal,
            outcome: ReadItemOutcome::Decoded(Box::new(record)),
        };
        let error = normalize_token_item(&source, &item).expect_err("timestamp should fail");
        assert_eq!(
            error,
            TokenNormalizationError::InvalidTimestamp(TimestampErrorReason::WrongShape)
        );
        assert!(!error.to_string().contains("not-a-real-source-timestamp"));
    }

    #[test]
    fn serialized_event_matches_codex_fixture_payload_and_timestamp() {
        let source = source("generation-1");
        let item = usage_item(TOKEN_USAGE_FIXTURE, 10, 100);
        let TokenNormalizationOutcome::PerResponse {
            normalized_event, ..
        } = normalize_token_item(&source, &item).expect("normalization should succeed")
        else {
            panic!("expected normalized event");
        };
        let actual = serde_json::to_value(normalized_event).expect("event should serialize");
        let expected: Value = serde_json::from_str(CODEX_NORMALIZED_FIXTURE)
            .expect("normalized fixture should be valid JSON");
        assert_eq!(actual["event_type"], expected["event_type"]);
        assert_eq!(actual["event_at"], expected["event_at"]);
        assert_eq!(actual["payload"], expected["payload"]);
        assert_eq!(
            actual["payload"]["token_counters"]["uncached_input"]["availability"],
            "unavailable"
        );
    }

    fn normalized_event(source: &SourceIdentity, item: &ReadItem) -> NormalizedTokenEvent {
        let TokenNormalizationOutcome::PerResponse {
            normalized_event, ..
        } = normalize_token_item(source, item).expect("normalization should succeed")
        else {
            panic!("expected normalized event");
        };
        *normalized_event
    }
}
