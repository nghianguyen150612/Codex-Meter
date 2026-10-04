//! Stable, privacy-safe identity derivation shared by telemetry layers.

use sha2::{Digest, Sha256};

use super::incremental_jsonl::SourceIdentity;

pub(crate) fn source_instance_id(source: &SourceIdentity) -> String {
    derive_id(
        "src:",
        "codex-meter/source-instance/v1",
        &[source.rollout_id(), source.source_generation()],
    )
}

pub(crate) fn token_event_id(
    source: &SourceIdentity,
    start_offset: u64,
    end_offset: u64,
    ordinal: Option<u64>,
) -> String {
    derive_position_id(
        "evt:",
        "codex-meter/token-event/v1",
        source,
        start_offset,
        end_offset,
        ordinal,
    )
}

pub(crate) fn cursor_id(source: &SourceIdentity, end_offset: u64) -> String {
    derive_id(
        "cursor:",
        "codex-meter/source-boundary/v1",
        &[
            source.rollout_id(),
            source.source_generation(),
            &end_offset.to_string(),
        ],
    )
}

pub(crate) fn session_id(upstream_id: &str) -> String {
    derive_id("session:", "codex-meter/session/v1", &[upstream_id])
}

pub(crate) fn task_id(upstream_id: &str) -> String {
    derive_id("task:", "codex-meter/task/v1", &[upstream_id])
}

pub(crate) fn thread_id(upstream_id: &str) -> String {
    derive_id("thread:", "codex-meter/thread/v1", &[upstream_id])
}

pub(crate) fn session_detected_event_id(
    source: &SourceIdentity,
    start_offset: u64,
    end_offset: u64,
    ordinal: Option<u64>,
) -> String {
    derive_position_id(
        "evt:",
        "codex-meter/session-detected-event/v1",
        source,
        start_offset,
        end_offset,
        ordinal,
    )
}

pub(crate) fn configuration_event_id(
    source: &SourceIdentity,
    start_offset: u64,
    end_offset: u64,
    ordinal: Option<u64>,
) -> String {
    derive_position_id(
        "evt:",
        "codex-meter/configuration-event/v1",
        source,
        start_offset,
        end_offset,
        ordinal,
    )
}

pub(crate) fn lifecycle_event_id(
    source: &SourceIdentity,
    start_offset: u64,
    end_offset: u64,
    ordinal: Option<u64>,
) -> String {
    derive_position_id(
        "evt:",
        "codex-meter/task-lifecycle/v1",
        source,
        start_offset,
        end_offset,
        ordinal,
    )
}

pub(crate) fn snapshot_evidence_id(
    source: &SourceIdentity,
    start_offset: u64,
    end_offset: u64,
    ordinal: Option<u64>,
) -> String {
    derive_position_id(
        "evidence:",
        "codex-meter/token-snapshot/v1",
        source,
        start_offset,
        end_offset,
        ordinal,
    )
}

pub(crate) fn quota_sample_id(
    source: &SourceIdentity,
    start_offset: u64,
    end_offset: u64,
    ordinal: Option<u64>,
    slot: &str,
    meter_type: &str,
) -> String {
    let ordinal = ordinal.map_or_else(String::new, |value| value.to_string());
    derive_id(
        "quota:",
        "codex-meter/quota-sample/v1",
        &[
            source.rollout_id(),
            source.source_generation(),
            &start_offset.to_string(),
            &end_offset.to_string(),
            &ordinal,
            slot,
            meter_type,
        ],
    )
}

pub(crate) fn local_quota_window_id(meter_type: &str, anchor_sample_id: &str) -> String {
    derive_id(
        "window:",
        "codex-meter/quota-window/v1",
        &[meter_type, anchor_sample_id],
    )
}

pub(crate) fn configuration_fingerprint(fields: &[Option<&str>]) -> String {
    let parts: Vec<String> = fields
        .iter()
        .map(|field| field.map_or_else(|| "<missing>".to_owned(), ToOwned::to_owned))
        .collect();
    let references: Vec<&str> = parts.iter().map(String::as_str).collect();
    derive_id(
        "cfg:",
        "codex-meter/configuration-fingerprint/v1",
        &references,
    )
}

pub(crate) fn observation_id(
    task_id: &str,
    session_id: Option<&str>,
    ended_at: Option<&str>,
) -> String {
    derive_id(
        "obs:",
        "codex-meter/observation/v1",
        &[
            task_id,
            session_id.unwrap_or("<missing>"),
            ended_at.unwrap_or("<missing>"),
        ],
    )
}

fn derive_position_id(
    prefix: &str,
    domain: &str,
    source: &SourceIdentity,
    start_offset: u64,
    end_offset: u64,
    ordinal: Option<u64>,
) -> String {
    let ordinal = ordinal.map_or_else(String::new, |value| value.to_string());
    derive_id(
        prefix,
        domain,
        &[
            source.rollout_id(),
            source.source_generation(),
            &start_offset.to_string(),
            &end_offset.to_string(),
            &ordinal,
        ],
    )
}

fn derive_id(prefix: &str, domain: &str, parts: &[&str]) -> String {
    let mut hasher = Sha256::new();
    hash_part(&mut hasher, domain);
    for part in parts {
        hash_part(&mut hasher, part);
    }
    let digest = hasher.finalize();
    let mut result = String::with_capacity(prefix.len() + digest.len() * 2);
    result.push_str(prefix);
    for byte in digest {
        result.push_str(&format!("{byte:02x}"));
    }
    result
}

fn hash_part(hasher: &mut Sha256, part: &str) {
    hasher.update((part.len() as u64).to_be_bytes());
    hasher.update(part.as_bytes());
}
