//! Shared synthetic evidence through the existing P039 checksum fixtures.
use crate::storage::*;
use std::sync::atomic::{AtomicU64, Ordering};
static NEXT: AtomicU64 = AtomicU64::new(0);

pub(super) struct Fixture(pub std::path::PathBuf);
impl Fixture {
    pub fn new(events: &[TokenUsageEvidence]) -> Self {
        let path = std::env::temp_dir().join(format!(
            "codex-meter-p040-{}-{}.sqlite",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let store = SqliteStore::open(&path).unwrap();
        for event in events {
            insert_test_token(&store, event);
        }
        for (index, code) in [
            IngestionDiagnosticCode::UnsupportedCodexVersion,
            IngestionDiagnosticCode::DataCoverageIncomplete,
            IngestionDiagnosticCode::MalformedRecord,
            IngestionDiagnosticCode::CompressedSourceUnsupported,
            IngestionDiagnosticCode::SourceGenerationAmbiguous,
        ]
        .into_iter()
        .enumerate()
        {
            insert_test_diagnostic(
                &store,
                &IngestionDiagnostic {
                    diagnostic_id: format!("diag:{index}"),
                    source_id: "src:fixture".into(),
                    observed_at: None,
                    record_semantic: "source_diagnostic".into(),
                    diagnostic_code: code,
                },
            );
        }
        Self(path)
    }
    pub fn report(&self) -> TaskUsageReport {
        SqliteReadStore::open(&self.0)
            .unwrap()
            .task_usage(&TaskUsageQuery {
                since: None,
                until: None,
                model: None,
                session_id: None,
                limit: 200,
            })
            .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
        let _ = std::fs::remove_file(self.0.with_extension("sqlite-wal"));
        let _ = std::fs::remove_file(self.0.with_extension("sqlite-shm"));
    }
}
pub(super) fn event(index: usize) -> TokenUsageEvidence {
    test_token(
        &format!("evt:{index}"),
        "2026-10-20T12:00:00Z",
        Some("model-fixture"),
        Some("session:fixture"),
        TokenRecordSemantic::PerResponse,
        Some(index as u64),
        Some(20),
        Some(10),
        Some(4),
    )
}
pub(super) fn report(count: usize) -> TaskUsageReport {
    Fixture::new(&(0..count).map(event).collect::<Vec<_>>()).report()
}
