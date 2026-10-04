//! Incremental reader for one plain Codex rollout JSONL source.
//!
//! This module owns newline boundaries and replay position. It deliberately
//! does not discover files, watch directories, decompress rollout streams, or
//! normalize decoded source records.

use std::fmt;
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom};

use super::codex_rollout::{parse_rollout_record, RolloutDecodeError, RolloutRecord};

const MAX_ID_LENGTH: usize = 128;

/// Opaque caller-supplied identity for one logical rollout and its physical generation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceIdentity {
    rollout_id: String,
    source_generation: String,
}

impl SourceIdentity {
    /// Creates a source identity without inspecting or deriving physical file metadata.
    pub fn new(
        rollout_id: impl Into<String>,
        source_generation: impl Into<String>,
    ) -> Result<Self, CursorIdentityError> {
        let rollout_id = validate_identity("rollout_id", rollout_id.into())?;
        let source_generation = validate_identity("source_generation", source_generation.into())?;
        Ok(Self {
            rollout_id,
            source_generation,
        })
    }

    /// Returns the opaque logical rollout identifier.
    pub fn rollout_id(&self) -> &str {
        &self.rollout_id
    }

    /// Returns the opaque physical-generation identifier.
    pub fn source_generation(&self) -> &str {
        &self.source_generation
    }
}

/// Candidate replay position for one source generation.
///
/// `committed_offset` points to the first byte that has not been committed as
/// part of a complete newline-terminated record. The reader never mutates a
/// cursor supplied by its caller; callers decide when to retain the returned
/// candidate cursor.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RolloutCursor {
    rollout_id: String,
    source_generation: String,
    committed_offset: u64,
    last_ordinal: Option<u64>,
}

impl RolloutCursor {
    /// Creates a cursor at the beginning of a new source generation.
    pub fn at_start(source: &SourceIdentity) -> Self {
        Self {
            rollout_id: source.rollout_id.clone(),
            source_generation: source.source_generation.clone(),
            committed_offset: 0,
            last_ordinal: None,
        }
    }

    /// Restores a validated cursor from durable checkpoint fields.
    pub fn from_checkpoint(
        source: &SourceIdentity,
        committed_offset: u64,
        last_ordinal: Option<u64>,
    ) -> Self {
        Self {
            rollout_id: source.rollout_id.clone(),
            source_generation: source.source_generation.clone(),
            committed_offset,
            last_ordinal,
        }
    }

    /// Returns the logical rollout identifier carried by this cursor.
    pub fn rollout_id(&self) -> &str {
        &self.rollout_id
    }

    /// Returns the source-generation identifier carried by this cursor.
    pub fn source_generation(&self) -> &str {
        &self.source_generation
    }

    /// Returns the first uncommitted byte offset.
    pub fn committed_offset(&self) -> u64 {
        self.committed_offset
    }

    /// Returns the greatest accepted ordinal, when any accepted record had one.
    pub fn last_ordinal(&self) -> Option<u64> {
        self.last_ordinal
    }
}

/// All outcomes produced for complete lines in one reader invocation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReadBatch {
    pub items: Vec<ReadItem>,
    pub next_cursor: RolloutCursor,
    pub has_incomplete_tail: bool,
}

/// Safe result for one complete newline-terminated line.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReadItem {
    pub start_offset: u64,
    pub end_offset: u64,
    pub ordinal: Option<u64>,
    pub outcome: ReadItemOutcome,
}

/// A decoded source record or a safely classified rejected line.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReadItemOutcome {
    Decoded(Box<RolloutRecord>),
    Rejected(RejectedLine),
}

/// Structural reason a complete line was not decoded.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RejectedLine {
    pub reason: RejectedLineReason,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RejectedLineReason {
    EmptyLine,
    InvalidUtf8,
    Decode(RolloutDecodeError),
}

/// Failures that prevent returning a candidate batch.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum IncrementalReadError {
    Io {
        operation: IoOperation,
        kind: io::ErrorKind,
    },
    CursorIdentityInvalid(CursorIdentityError),
    RolloutMismatch {
        cursor_rollout_id: String,
        source_rollout_id: String,
    },
    SourceReplaced {
        cursor_generation: String,
        source_generation: String,
    },
    SourceTruncated {
        committed_offset: u64,
        current_length: u64,
    },
    OffsetOverflow {
        offset: u64,
        line_length: usize,
    },
    OrdinalRegression {
        previous: u64,
        observed: u64,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IoOperation {
    SeekToEnd,
    SeekToOffset,
    ReadLine,
}

/// Errors from validating caller-supplied opaque identities.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CursorIdentityError {
    Empty { field: &'static str },
    TooLong { field: &'static str },
    ControlCharacter { field: &'static str },
}

impl fmt::Display for CursorIdentityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty { field } => write!(formatter, "{field} must not be empty"),
            Self::TooLong { field } => write!(formatter, "{field} exceeds the bounded length"),
            Self::ControlCharacter { field } => {
                write!(formatter, "{field} contains a control character")
            }
        }
    }
}

impl std::error::Error for CursorIdentityError {}

impl fmt::Display for IncrementalReadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { operation, kind } => write!(
                formatter,
                "rollout {operation:?} failed with I/O kind {kind:?}"
            ),
            Self::CursorIdentityInvalid(error) => write!(formatter, "invalid cursor identity: {error}"),
            Self::RolloutMismatch {
                cursor_rollout_id,
                source_rollout_id,
            } => write!(
                formatter,
                "cursor rollout {cursor_rollout_id:?} does not match source rollout {source_rollout_id:?}"
            ),
            Self::SourceReplaced {
                cursor_generation,
                source_generation,
            } => write!(
                formatter,
                "source generation changed from {cursor_generation:?} to {source_generation:?}"
            ),
            Self::SourceTruncated {
                committed_offset,
                current_length,
            } => write!(
                formatter,
                "source length {current_length} is below committed offset {committed_offset}"
            ),
            Self::OffsetOverflow {
                offset,
                line_length,
            } => write!(
                formatter,
                "source offset {offset} cannot advance by line length {line_length}"
            ),
            Self::OrdinalRegression { previous, observed } => write!(
                formatter,
                "ordinal regressed from {previous} to {observed}"
            ),
        }
    }
}

impl std::error::Error for IncrementalReadError {}

/// Reads currently available complete lines from a plain JSONL source.
///
/// The returned cursor is only a candidate. A caller should retain it only
/// after processing every returned item successfully. A complete malformed or
/// empty line advances the candidate offset, while an unterminated EOF tail
/// does not.
pub fn read_available<R: Read + Seek>(
    reader: &mut R,
    source: &SourceIdentity,
    cursor: &RolloutCursor,
) -> Result<ReadBatch, IncrementalReadError> {
    validate_cursor_identity(cursor)?;
    if cursor.rollout_id != source.rollout_id {
        return Err(IncrementalReadError::RolloutMismatch {
            cursor_rollout_id: cursor.rollout_id.clone(),
            source_rollout_id: source.rollout_id.clone(),
        });
    }
    if cursor.source_generation != source.source_generation {
        return Err(IncrementalReadError::SourceReplaced {
            cursor_generation: cursor.source_generation.clone(),
            source_generation: source.source_generation.clone(),
        });
    }

    let current_length = reader
        .seek(SeekFrom::End(0))
        .map_err(|error| io_error(IoOperation::SeekToEnd, error))?;
    if current_length < cursor.committed_offset {
        return Err(IncrementalReadError::SourceTruncated {
            committed_offset: cursor.committed_offset,
            current_length,
        });
    }
    reader
        .seek(SeekFrom::Start(cursor.committed_offset))
        .map_err(|error| io_error(IoOperation::SeekToOffset, error))?;

    let mut buffered = BufReader::new(reader);
    let mut next_cursor = cursor.clone();
    let mut items = Vec::new();
    let mut has_incomplete_tail = false;
    let mut line = Vec::new();

    loop {
        line.clear();
        let start_offset = next_cursor.committed_offset;
        let bytes_read = buffered
            .read_until(b'\n', &mut line)
            .map_err(|error| io_error(IoOperation::ReadLine, error))?;
        if bytes_read == 0 {
            break;
        }

        let line_length =
            u64::try_from(bytes_read).map_err(|_| IncrementalReadError::OffsetOverflow {
                offset: start_offset,
                line_length: bytes_read,
            })?;
        let end_offset =
            start_offset
                .checked_add(line_length)
                .ok_or(IncrementalReadError::OffsetOverflow {
                    offset: start_offset,
                    line_length: bytes_read,
                })?;
        if line.last() != Some(&b'\n') {
            has_incomplete_tail = true;
            break;
        }

        next_cursor.committed_offset = end_offset;
        let record_bytes = &line[..line.len() - 1];
        let record_bytes = record_bytes.strip_suffix(b"\r").unwrap_or(record_bytes);
        let outcome = if record_bytes.is_empty() {
            ReadItemOutcome::Rejected(RejectedLine {
                reason: RejectedLineReason::EmptyLine,
            })
        } else {
            match std::str::from_utf8(record_bytes) {
                Ok(record) => match parse_rollout_record(record) {
                    Ok(record) => {
                        if let Some(observed) = record.ordinal {
                            if let Some(previous) = next_cursor.last_ordinal {
                                if observed <= previous {
                                    return Err(IncrementalReadError::OrdinalRegression {
                                        previous,
                                        observed,
                                    });
                                }
                            }
                            next_cursor.last_ordinal = Some(observed);
                        }
                        ReadItemOutcome::Decoded(Box::new(record))
                    }
                    Err(error) => ReadItemOutcome::Rejected(RejectedLine {
                        reason: RejectedLineReason::Decode(error),
                    }),
                },
                Err(_) => ReadItemOutcome::Rejected(RejectedLine {
                    reason: RejectedLineReason::InvalidUtf8,
                }),
            }
        };
        let ordinal = match &outcome {
            ReadItemOutcome::Decoded(record) => record.ordinal,
            ReadItemOutcome::Rejected(_) => None,
        };
        items.push(ReadItem {
            start_offset,
            end_offset,
            ordinal,
            outcome,
        });
    }

    Ok(ReadBatch {
        items,
        next_cursor,
        has_incomplete_tail,
    })
}

fn validate_cursor_identity(cursor: &RolloutCursor) -> Result<(), IncrementalReadError> {
    validate_identity("rollout_id", cursor.rollout_id.clone())
        .and_then(|_| validate_identity("source_generation", cursor.source_generation.clone()))
        .map(|_| ())
        .map_err(IncrementalReadError::CursorIdentityInvalid)
}

fn validate_identity(field: &'static str, value: String) -> Result<String, CursorIdentityError> {
    if value.is_empty() {
        return Err(CursorIdentityError::Empty { field });
    }
    if value.len() > MAX_ID_LENGTH {
        return Err(CursorIdentityError::TooLong { field });
    }
    if value.chars().any(char::is_control) {
        return Err(CursorIdentityError::ControlCharacter { field });
    }
    Ok(value)
}

fn io_error(operation: IoOperation, error: io::Error) -> IncrementalReadError {
    IncrementalReadError::Io {
        operation,
        kind: error.kind(),
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    const SESSION_META: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../fixtures/codex-rollout/v0.157.1/session-meta.json"
    ));
    const TOKEN_USAGE: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../fixtures/codex-rollout/v0.157.1/token-usage-record.json"
    ));

    fn source(generation: &str) -> SourceIdentity {
        SourceIdentity::new("rollout-synthetic-001", generation).expect("valid source identity")
    }

    fn line(record: &str) -> String {
        format!("{}\n", record.trim_end_matches(['\r', '\n']))
    }

    fn decoded_count(batch: &ReadBatch) -> usize {
        batch
            .items
            .iter()
            .filter(|item| matches!(item.outcome, ReadItemOutcome::Decoded(_)))
            .count()
    }

    #[test]
    fn reads_multiple_complete_records_and_tracks_offsets_and_ordinals() {
        let source = source("generation-1");
        let bytes = format!(
            "{}{}",
            line(&SESSION_META.replace("\"ordinal\":0", "\"ordinal\":1")),
            line(&TOKEN_USAGE.replace("\"ordinal\":12", "\"ordinal\":2"))
        );
        let mut reader = Cursor::new(bytes.as_bytes());
        let batch = read_available(&mut reader, &source, &RolloutCursor::at_start(&source))
            .expect("complete records should read");

        assert_eq!(decoded_count(&batch), 2);
        assert!(!batch.has_incomplete_tail);
        assert_eq!(batch.next_cursor.committed_offset(), bytes.len() as u64);
        assert_eq!(batch.next_cursor.last_ordinal(), Some(2));
        assert_eq!(batch.items[0].start_offset, 0);
        assert_eq!(batch.items[1].start_offset, batch.items[0].end_offset);
    }

    #[test]
    fn partial_tail_is_not_parsed_or_committed_then_completes_on_append() {
        let source = source("generation-1");
        let complete_a = line(&SESSION_META.replace("\"ordinal\":0", "\"ordinal\":1"));
        let partial_b = TOKEN_USAGE
            .replace("\"ordinal\":12", "\"ordinal\":2")
            .trim_end_matches(['\r', '\n'])
            .to_owned();
        let mut bytes = format!("{complete_a}{partial_b}").into_bytes();
        let mut reader = Cursor::new(bytes.clone());
        let first = read_available(&mut reader, &source, &RolloutCursor::at_start(&source))
            .expect("first read should succeed");

        assert_eq!(decoded_count(&first), 1);
        assert!(first.has_incomplete_tail);
        assert_eq!(
            first.next_cursor.committed_offset(),
            complete_a.len() as u64
        );
        assert_eq!(first.next_cursor.last_ordinal(), Some(1));

        bytes.extend_from_slice(b"\n");
        reader = Cursor::new(bytes);
        let second = read_available(&mut reader, &source, &first.next_cursor)
            .expect("completed append should read");
        assert_eq!(decoded_count(&second), 1);
        assert!(!second.has_incomplete_tail);
        assert_eq!(second.next_cursor.last_ordinal(), Some(2));
    }

    #[test]
    fn committed_cursor_retries_empty_and_old_cursor_replays() {
        let source = source("generation-1");
        let bytes = format!("{}{}", line(SESSION_META), line(TOKEN_USAGE));
        let mut reader = Cursor::new(bytes.as_bytes());
        let initial = RolloutCursor::at_start(&source);
        let batch = read_available(&mut reader, &source, &initial).expect("read should succeed");

        let mut same_reader = Cursor::new(bytes.as_bytes());
        let empty = read_available(&mut same_reader, &source, &batch.next_cursor)
            .expect("committed replay should succeed");
        assert!(empty.items.is_empty());
        assert_eq!(empty.next_cursor, batch.next_cursor);

        let mut replay_reader = Cursor::new(bytes.as_bytes());
        let replay = read_available(&mut replay_reader, &source, &initial)
            .expect("old cursor should replay");
        assert_eq!(replay.items, batch.items);
        assert_eq!(replay.next_cursor, batch.next_cursor);
    }

    #[test]
    fn malformed_complete_line_is_rejected_and_later_records_are_read() {
        let source = source("generation-1");
        let malformed = "{synthetic malformed payload that must not appear}";
        let bytes = format!(
            "{}{}{}",
            line(SESSION_META),
            line(malformed),
            line(TOKEN_USAGE)
        );
        let mut reader = Cursor::new(bytes.as_bytes());
        let batch = read_available(&mut reader, &source, &RolloutCursor::at_start(&source))
            .expect("malformed complete line should not abort batch");

        assert_eq!(batch.items.len(), 3);
        assert!(matches!(
            batch.items[1].outcome,
            ReadItemOutcome::Rejected(RejectedLine {
                reason: RejectedLineReason::Decode(RolloutDecodeError::InvalidJson { .. })
            })
        ));
        assert_eq!(decoded_count(&batch), 2);
        assert!(!format!("{:?}", batch.items[1]).contains("synthetic malformed payload"));
    }

    #[test]
    fn empty_line_is_rejected_and_offset_advances() {
        let source = source("generation-1");
        let bytes = format!("{}\n{}", line(SESSION_META), line(TOKEN_USAGE));
        let mut reader = Cursor::new(bytes.as_bytes());
        let batch = read_available(&mut reader, &source, &RolloutCursor::at_start(&source))
            .expect("empty line should be a rejected item");

        assert!(matches!(
            batch.items[1].outcome,
            ReadItemOutcome::Rejected(RejectedLine {
                reason: RejectedLineReason::EmptyLine
            })
        ));
        assert_eq!(decoded_count(&batch), 2);
        assert_eq!(batch.next_cursor.committed_offset(), bytes.len() as u64);
    }

    #[test]
    fn crlf_strips_carriage_return_and_counts_both_newline_bytes() {
        let source = source("generation-1");
        let bytes = format!("{}\r\n{}\r\n", SESSION_META, TOKEN_USAGE);
        let mut reader = Cursor::new(bytes.as_bytes());
        let batch = read_available(&mut reader, &source, &RolloutCursor::at_start(&source))
            .expect("CRLF records should read");

        assert_eq!(decoded_count(&batch), 2);
        assert_eq!(batch.next_cursor.committed_offset(), bytes.len() as u64);
    }

    #[test]
    fn ordinal_regression_is_a_fatal_discontinuity() {
        let source = source("generation-1");
        let first = SESSION_META.replace("\"ordinal\":0", "\"ordinal\":4");
        let second = TOKEN_USAGE.replace("\"ordinal\":12", "\"ordinal\":4");
        let bytes = format!("{}{}", line(&first), line(&second));
        let mut reader = Cursor::new(bytes.as_bytes());
        let error = read_available(&mut reader, &source, &RolloutCursor::at_start(&source))
            .expect_err("ordinal regression should be reported");

        assert_eq!(
            error,
            IncrementalReadError::OrdinalRegression {
                previous: 4,
                observed: 4
            }
        );
    }

    #[test]
    fn missing_ordinals_do_not_become_zero_or_reset_progress() {
        let source = source("generation-1");
        let first = SESSION_META.replace("\"ordinal\":0,", "");
        let second = TOKEN_USAGE.replace("\"ordinal\":12", "\"ordinal\":5");
        let bytes = format!("{}{}", line(&first), line(&second));
        let mut reader = Cursor::new(bytes.as_bytes());
        let batch = read_available(&mut reader, &source, &RolloutCursor::at_start(&source))
            .expect("missing ordinal should be accepted");

        assert_eq!(batch.items[0].ordinal, None);
        assert_eq!(batch.items[1].ordinal, Some(5));
        assert_eq!(batch.next_cursor.last_ordinal(), Some(5));
    }

    #[test]
    fn truncation_is_explicit_and_does_not_restart_at_zero() {
        let source = source("generation-1");
        let cursor = RolloutCursor {
            rollout_id: source.rollout_id.clone(),
            source_generation: source.source_generation.clone(),
            committed_offset: 20,
            last_ordinal: Some(7),
        };
        let mut reader = Cursor::new(b"short".to_vec());
        let error = read_available(&mut reader, &source, &cursor)
            .expect_err("shorter source should be reported");

        assert_eq!(
            error,
            IncrementalReadError::SourceTruncated {
                committed_offset: 20,
                current_length: 5
            }
        );
    }

    #[test]
    fn generation_change_is_explicit() {
        let old_source = source("generation-1");
        let new_source = source("generation-2");
        let cursor = RolloutCursor::at_start(&old_source);
        let mut reader = Cursor::new(Vec::<u8>::new());
        let error = read_available(&mut reader, &new_source, &cursor)
            .expect_err("generation change should be reported");

        assert!(matches!(error, IncrementalReadError::SourceReplaced { .. }));
    }

    #[test]
    fn restart_helper_clears_offset_and_ordinal() {
        let source = source("generation-new");
        let cursor = RolloutCursor::at_start(&source);

        assert_eq!(cursor.committed_offset(), 0);
        assert_eq!(cursor.last_ordinal(), None);
        assert_eq!(cursor.source_generation(), "generation-new");
    }

    #[test]
    fn identity_values_are_bounded_and_non_content_errors_are_safe() {
        assert!(matches!(
            SourceIdentity::new("", "generation-1"),
            Err(CursorIdentityError::Empty {
                field: "rollout_id"
            })
        ));
        assert!(matches!(
            SourceIdentity::new("rollout\n", "generation-1"),
            Err(CursorIdentityError::ControlCharacter {
                field: "rollout_id"
            })
        ));
        assert!(matches!(
            SourceIdentity::new("rollout-1", "x".repeat(MAX_ID_LENGTH + 1)),
            Err(CursorIdentityError::TooLong {
                field: "source_generation"
            })
        ));
    }

    #[test]
    fn fixture_stream_files_are_consumable() {
        let source = source("generation-fixture");
        let complete = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../fixtures/codex-rollout/v0.157.1/streams/multiple-complete.jsonl"
        ));
        let mut reader = Cursor::new(complete);
        let batch = read_available(&mut reader, &source, &RolloutCursor::at_start(&source))
            .expect("fixture stream should read");
        assert_eq!(decoded_count(&batch), 2);

        let crlf = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../fixtures/codex-rollout/v0.157.1/streams/crlf.jsonl"
        ));
        let mut reader = Cursor::new(crlf);
        let batch = read_available(&mut reader, &source, &RolloutCursor::at_start(&source))
            .expect("CRLF fixture stream should read");
        assert_eq!(decoded_count(&batch), 2);

        let progression = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../fixtures/codex-rollout/v0.157.1/streams/ordinal-progression.jsonl"
        ));
        let mut reader = Cursor::new(progression);
        let batch = read_available(&mut reader, &source, &RolloutCursor::at_start(&source))
            .expect("ordinal gaps should be accepted");
        assert_eq!(batch.next_cursor.last_ordinal(), Some(12));

        let malformed_and_empty = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../fixtures/codex-rollout/v0.157.1/streams/malformed-and-empty.jsonl"
        ));
        let mut reader = Cursor::new(malformed_and_empty);
        let batch = read_available(&mut reader, &source, &RolloutCursor::at_start(&source))
            .expect("malformed and empty lines should be classified");
        assert_eq!(batch.items.len(), 4);
    }
}
