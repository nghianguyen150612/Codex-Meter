use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration as StdDuration;

use time::{format_description::well_known::Rfc3339, Duration, OffsetDateTime, Time};

use crate::cli::{self, TuiArguments};
use crate::codex_app_server::{
    ApprovalChoice, CodexAppError, CodexAppServer, ControllerEvent, ExecutionPreflight,
    InteractiveExecution, ModelCatalog, ProjectSummary, SessionDetail, SessionListOptions,
    SessionMetadata,
};
use crate::storage::{
    EvidenceDetail, EvidenceQualitySummary, QuotaHistoryCursor, QuotaHistoryPage, QuotaTimeline,
    SqliteReadStore, StorageError, TokenHistoryCursor, TokenHistoryPage, TokenHistoryQuery,
    TokenStatistics, TokenStatisticsQuery,
};
use crate::tui::composer::PreparedTask;

use super::state::HistoryScope;

const WORKER_QUEUE_CAPACITY: usize = 8;
const RESULT_QUEUE_CAPACITY: usize = 32;
const PAGE_SIZE: u32 = 50;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LedgerStatus {
    Missing,
    Available,
}

#[derive(Clone, Debug)]
pub struct OverviewData {
    pub ledger_status: LedgerStatus,
    pub token_statistics: Option<TokenStatistics>,
    pub quota: Option<QuotaTimeline>,
    pub evidence_quality: Option<EvidenceQualitySummary>,
    pub telemetry_version: Option<String>,
    pub schema_version: Option<u32>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MutationKind {
    Resume,
    Fork,
    Sync,
}

pub enum BackendEvent {
    Overview {
        id: u64,
        result: Result<Box<OverviewData>, String>,
    },
    Sessions {
        id: u64,
        result: Result<Vec<SessionMetadata>, String>,
    },
    SessionDetail {
        id: u64,
        result: Result<SessionDetail, String>,
    },
    Projects {
        id: u64,
        result: Result<Vec<ProjectSummary>, String>,
    },
    Models {
        id: u64,
        result: Result<ModelCatalog, String>,
    },
    History {
        id: u64,
        result: Result<TokenHistoryPage, String>,
    },
    QuotaHistory {
        id: u64,
        result: Result<QuotaHistoryPage, String>,
    },
    EvidenceDetail {
        id: u64,
        result: Result<EvidenceDetail, String>,
    },
    Mutation {
        id: u64,
        kind: MutationKind,
        result: Result<String, String>,
    },
    ExecutionPreflight {
        id: u64,
        result: Result<ExecutionPreflight, String>,
    },
    ExecutionProgress {
        id: u64,
        event: ControllerEvent,
    },
    ExecutionActionComplete {
        id: u64,
    },
    ExecutionCancelled {
        id: u64,
    },
}

enum DatabaseCommand {
    Overview {
        id: u64,
    },
    History {
        id: u64,
        scope: HistoryScope,
        model: Option<String>,
        cursor: Option<TokenHistoryCursor>,
    },
    QuotaHistory {
        id: u64,
        cursor: Option<QuotaHistoryCursor>,
    },
    Inspect {
        id: u64,
        evidence_id: String,
    },
    Sync {
        id: u64,
    },
}

enum AppServerCommand {
    Sessions {
        id: u64,
    },
    SessionDetail {
        id: u64,
        thread_id: String,
    },
    Projects {
        id: u64,
    },
    Models {
        id: u64,
    },
    Resume {
        id: u64,
        thread_id: String,
    },
    Fork {
        id: u64,
        thread_id: String,
    },
    PrepareExecution {
        id: u64,
        task: Box<PreparedTask>,
    },
    InspectExecution {
        id: u64,
    },
    SubmitExecution {
        id: u64,
    },
    DecideApproval {
        id: u64,
        request_id: String,
        choice: ApprovalChoice,
    },
    InterruptExecution {
        id: u64,
    },
    ReconcileExecution {
        id: u64,
    },
    CancelExecution {
        id: u64,
    },
}

pub struct Backend {
    database_sender: SyncSender<DatabaseCommand>,
    app_server_sender: SyncSender<AppServerCommand>,
    result_receiver: Receiver<BackendEvent>,
    shutdown: Arc<AtomicBool>,
    worker_handles: Vec<JoinHandle<()>>,
}

type AppServerConnector = fn() -> Result<CodexAppServer, CodexAppError>;

#[cfg(test)]
#[derive(Clone, Copy)]
pub(super) enum TestAppServerMode {
    Sessions,
    Models,
}

impl Backend {
    pub fn new(arguments: &TuiArguments, database_path: PathBuf) -> Result<Self, String> {
        Self::new_with_connector(arguments, database_path, CodexAppServer::connect)
    }

    fn new_with_connector(
        arguments: &TuiArguments,
        database_path: PathBuf,
        app_server_connector: AppServerConnector,
    ) -> Result<Self, String> {
        let (database_sender, database_receiver) = mpsc::sync_channel(WORKER_QUEUE_CAPACITY);
        let (app_server_sender, app_server_receiver) = mpsc::sync_channel(WORKER_QUEUE_CAPACITY);
        let (result_sender, result_receiver) = mpsc::sync_channel(RESULT_QUEUE_CAPACITY);
        let sync_database_override = arguments.database.clone();
        let shutdown = Arc::new(AtomicBool::new(false));
        let database_result_sender = result_sender.clone();
        let database_shutdown = Arc::clone(&shutdown);
        let database_handle = thread::Builder::new()
            .name("codex-meter-sqlite-worker".to_owned())
            .spawn(move || {
                database_worker(
                    database_path,
                    sync_database_override,
                    database_receiver,
                    database_result_sender,
                    database_shutdown,
                )
            })
            .map_err(|_| "could not start the local reporting worker".to_owned())?;
        let app_server_shutdown = Arc::clone(&shutdown);
        let app_server_handle = match thread::Builder::new()
            .name("codex-meter-app-server-worker".to_owned())
            .spawn(move || {
                app_server_worker(
                    app_server_receiver,
                    result_sender,
                    app_server_connector,
                    app_server_shutdown,
                )
            }) {
            Ok(handle) => handle,
            Err(_) => {
                shutdown.store(true, Ordering::Release);
                let _ = database_handle.join();
                return Err("could not start the local App Server worker".to_owned());
            }
        };
        Ok(Self {
            database_sender,
            app_server_sender,
            result_receiver,
            shutdown,
            worker_handles: vec![database_handle, app_server_handle],
        })
    }

    #[cfg(test)]
    pub(super) fn new_test(
        database_path: PathBuf,
        mode: TestAppServerMode,
    ) -> Result<Self, String> {
        let arguments = TuiArguments {
            database: Some(database_path.clone()),
        };
        let connector = match mode {
            TestAppServerMode::Sessions => connect_test_sessions_app_server,
            TestAppServerMode::Models => connect_test_models_app_server,
        };
        Self::new_with_connector(&arguments, database_path, connector)
    }

    pub fn request_overview(&self, id: u64) -> Result<(), String> {
        self.send_database(DatabaseCommand::Overview { id })
    }

    pub fn request_sessions(&self, id: u64) -> Result<(), String> {
        self.send_app_server(AppServerCommand::Sessions { id })
    }

    pub fn request_session_detail(&self, id: u64, thread_id: String) -> Result<(), String> {
        self.send_app_server(AppServerCommand::SessionDetail { id, thread_id })
    }

    pub fn request_projects(&self, id: u64) -> Result<(), String> {
        self.send_app_server(AppServerCommand::Projects { id })
    }

    pub fn request_models(&self, id: u64) -> Result<(), String> {
        self.send_app_server(AppServerCommand::Models { id })
    }

    pub fn request_history(
        &self,
        id: u64,
        scope: HistoryScope,
        model: Option<String>,
        cursor: Option<TokenHistoryCursor>,
    ) -> Result<(), String> {
        self.send_database(DatabaseCommand::History {
            id,
            scope,
            model,
            cursor,
        })
    }

    pub fn request_quota_history(
        &self,
        id: u64,
        cursor: Option<QuotaHistoryCursor>,
    ) -> Result<(), String> {
        self.send_database(DatabaseCommand::QuotaHistory { id, cursor })
    }

    pub fn request_inspect(&self, id: u64, evidence_id: String) -> Result<(), String> {
        self.send_database(DatabaseCommand::Inspect { id, evidence_id })
    }

    pub fn request_resume(&self, id: u64, thread_id: String) -> Result<(), String> {
        self.send_app_server(AppServerCommand::Resume { id, thread_id })
    }

    pub fn request_fork(&self, id: u64, thread_id: String) -> Result<(), String> {
        self.send_app_server(AppServerCommand::Fork { id, thread_id })
    }

    pub fn prepare_execution(&self, id: u64, task: PreparedTask) -> Result<(), String> {
        self.send_app_server(AppServerCommand::PrepareExecution {
            id,
            task: Box::new(task),
        })
    }

    pub fn inspect_execution(&self, id: u64) -> Result<(), String> {
        self.send_app_server(AppServerCommand::InspectExecution { id })
    }

    pub fn submit_execution(&self, id: u64) -> Result<(), String> {
        self.send_app_server(AppServerCommand::SubmitExecution { id })
    }

    pub fn decide_approval(
        &self,
        id: u64,
        request_id: String,
        choice: ApprovalChoice,
    ) -> Result<(), String> {
        self.send_app_server(AppServerCommand::DecideApproval {
            id,
            request_id,
            choice,
        })
    }

    pub fn interrupt_execution(&self, id: u64) -> Result<(), String> {
        self.send_app_server(AppServerCommand::InterruptExecution { id })
    }

    pub fn reconcile_execution(&self, id: u64) -> Result<(), String> {
        self.send_app_server(AppServerCommand::ReconcileExecution { id })
    }

    pub fn cancel_execution(&self, id: u64) -> Result<(), String> {
        self.send_app_server(AppServerCommand::CancelExecution { id })
    }

    pub fn request_sync(&self, id: u64) -> Result<(), String> {
        self.send_database(DatabaseCommand::Sync { id })
    }

    pub fn try_recv(&self) -> Option<BackendEvent> {
        self.result_receiver.try_recv().ok()
    }

    fn send_database(&self, command: DatabaseCommand) -> Result<(), String> {
        send_bounded(&self.database_sender, command)
    }

    fn send_app_server(&self, command: AppServerCommand) -> Result<(), String> {
        send_bounded(&self.app_server_sender, command)
    }
}

impl Drop for Backend {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Release);
        // Workers check this flag between bounded requests. Joining lets the
        // App Server client drop its child process before the CLI returns.
        for handle in self.worker_handles.drain(..) {
            let _ = handle.join();
        }
    }
}

fn send_bounded<T>(sender: &SyncSender<T>, command: T) -> Result<(), String> {
    match sender.try_send(command) {
        Ok(()) => Ok(()),
        Err(TrySendError::Full(_)) => {
            Err("A request is already queued; wait for it to finish.".to_owned())
        }
        Err(TrySendError::Disconnected(_)) => {
            Err("The local worker is unavailable; refresh to retry.".to_owned())
        }
    }
}

fn database_worker(
    database_path: PathBuf,
    sync_database_override: Option<PathBuf>,
    receiver: Receiver<DatabaseCommand>,
    sender: SyncSender<BackendEvent>,
    shutdown: Arc<AtomicBool>,
) {
    while let Some(command) = receive_command(&receiver, &shutdown) {
        if shutdown.load(Ordering::Acquire) {
            return;
        }
        let event = match command {
            DatabaseCommand::Overview { id } => BackendEvent::Overview {
                id,
                result: load_overview(&database_path).map(Box::new),
            },
            DatabaseCommand::History {
                id,
                scope,
                model,
                cursor,
            } => BackendEvent::History {
                id,
                result: load_history(&database_path, scope, model.as_deref(), cursor.as_ref()),
            },
            DatabaseCommand::QuotaHistory { id, cursor } => BackendEvent::QuotaHistory {
                id,
                result: load_quota_history(&database_path, cursor.as_ref()),
            },
            DatabaseCommand::Inspect { id, evidence_id } => BackendEvent::EvidenceDetail {
                id,
                result: load_evidence_detail(&database_path, &evidence_id),
            },
            DatabaseCommand::Sync { id } => BackendEvent::Mutation {
                id,
                kind: MutationKind::Sync,
                result: run_sync(sync_database_override.as_deref()),
            },
        };
        if shutdown.load(Ordering::Acquire) {
            return;
        }
        if sender.send(event).is_err() {
            return;
        }
    }
}

fn load_overview(path: &PathBuf) -> Result<OverviewData, String> {
    match std::fs::metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(OverviewData {
                ledger_status: LedgerStatus::Missing,
                token_statistics: None,
                quota: None,
                evidence_quality: None,
                telemetry_version: None,
                schema_version: None,
            });
        }
        Err(_) => return Err("Local SQLite evidence ledger is unavailable.".to_owned()),
        Ok(metadata) if !metadata.is_file() => {
            return Err("Local SQLite evidence ledger is unavailable.".to_owned());
        }
        Ok(_) => {}
    }
    let mut store = SqliteReadStore::open(path).map_err(database_error)?;
    let token_statistics = store
        .token_statistics(&TokenStatisticsQuery::default())
        .map_err(database_error)?;
    let quota = store
        .quota_timeline(false, PAGE_SIZE, None)
        .map_err(database_error)?;
    let evidence_quality = store.evidence_quality().map_err(database_error)?;
    let latest_token = store
        .token_history(&TokenHistoryQuery {
            limit: 1,
            ..TokenHistoryQuery::default()
        })
        .map_err(database_error)?
        .events
        .into_iter()
        .next();
    let latest_evidence = [
        quota
            .five_hour
            .as_ref()
            .map(|sample| (sample.observed_at.as_str(), sample.codex_version.as_deref())),
        quota
            .weekly
            .as_ref()
            .map(|sample| (sample.observed_at.as_str(), sample.codex_version.as_deref())),
        latest_token
            .as_ref()
            .map(|event| (event.observed_at.as_str(), event.codex_version.as_deref())),
    ]
    .into_iter()
    .flatten()
    .max_by_key(|(observed_at, _)| OffsetDateTime::parse(observed_at, &Rfc3339).ok());
    let telemetry_version = latest_evidence.and_then(|(_, version)| version.map(str::to_owned));
    Ok(OverviewData {
        ledger_status: LedgerStatus::Available,
        token_statistics: Some(token_statistics),
        quota: Some(quota),
        evidence_quality: Some(evidence_quality),
        telemetry_version,
        schema_version: Some(store.info().latest_applied_migration),
    })
}

fn load_history(
    path: &PathBuf,
    scope: HistoryScope,
    model: Option<&str>,
    cursor: Option<&TokenHistoryCursor>,
) -> Result<TokenHistoryPage, String> {
    let mut store = open_read_store(path)?;
    let (since, until) = history_bounds(scope);
    store
        .token_history(&TokenHistoryQuery {
            since,
            until,
            model: model.map(str::to_owned),
            session_id: None,
            limit: PAGE_SIZE,
            cursor: cursor.cloned(),
        })
        .map_err(database_error)
}

fn load_quota_history(
    path: &PathBuf,
    cursor: Option<&QuotaHistoryCursor>,
) -> Result<QuotaHistoryPage, String> {
    let mut store = open_read_store(path)?;
    store
        .quota_timeline(true, PAGE_SIZE, cursor)
        .map_err(database_error)?
        .history
        .ok_or_else(|| "Quota history is unavailable in this database.".to_owned())
}

fn load_evidence_detail(path: &PathBuf, evidence_id: &str) -> Result<EvidenceDetail, String> {
    open_read_store(path)?
        .inspect(evidence_id)
        .map_err(database_error)
}

fn open_read_store(path: &PathBuf) -> Result<SqliteReadStore, String> {
    match std::fs::metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err("No local telemetry database is available. Run `codex-meter sync` to collect supported local evidence.".to_owned());
        }
        Err(_) => return Err("Local SQLite evidence ledger is unavailable.".to_owned()),
        Ok(metadata) if !metadata.is_file() => {
            return Err("Local SQLite evidence ledger is unavailable.".to_owned())
        }
        Ok(_) => {}
    }
    SqliteReadStore::open(path).map_err(database_error)
}

fn history_bounds(scope: HistoryScope) -> (Option<String>, Option<String>) {
    let now = OffsetDateTime::now_utc();
    let until = now.format(&Rfc3339).ok();
    let since_time = match scope {
        HistoryScope::Today => Some(now.date().with_time(Time::MIDNIGHT).assume_utc()),
        HistoryScope::Week => Some(now - Duration::days(7)),
        HistoryScope::All => None,
    };
    (
        since_time.and_then(|value| value.format(&Rfc3339).ok()),
        if scope == HistoryScope::All {
            None
        } else {
            until
        },
    )
}

fn database_error(error: StorageError) -> String {
    format!("Local SQLite reporting failed ({})", error.code())
}

fn run_sync(database_override: Option<&std::path::Path>) -> Result<String, String> {
    let mut arguments = vec![
        std::ffi::OsString::from("sync"),
        std::ffi::OsString::from("--json"),
    ];
    if let Some(path) = database_override {
        arguments.push(std::ffi::OsString::from("--database"));
        arguments.push(path.as_os_str().to_os_string());
    }
    let output = cli::run(arguments)
        .map_err(|error| format!("Local telemetry sync failed ({})", error.code))?;
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&output) else {
        return Ok("Local one-shot telemetry sync finished; summary was unavailable.".to_owned());
    };
    let status = value
        .get("status")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("finished");
    let responses = value
        .get("responses_added")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let warnings = value
        .get("warnings")
        .and_then(serde_json::Value::as_array)
        .map_or(0, Vec::len);
    Ok(format!("One-shot local sync {status}: {responses} response events added; {warnings} coverage warning(s)."))
}

fn app_server_worker(
    receiver: Receiver<AppServerCommand>,
    sender: SyncSender<BackendEvent>,
    connector: AppServerConnector,
    shutdown: Arc<AtomicBool>,
) {
    let mut client: Option<CodexAppServer> = None;
    let mut execution: Option<(u64, InteractiveExecution)> = None;
    loop {
        if shutdown.load(Ordering::Acquire) {
            return;
        }
        if let Some((execution_id, controller)) = execution.as_mut() {
            if !send_controller_events(&sender, *execution_id, controller.drain_events()) {
                return;
            }
        }
        let command = match receiver.recv_timeout(StdDuration::from_millis(100)) {
            Ok(command) => command,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        };
        match command {
            AppServerCommand::PrepareExecution { id, task } => {
                if execution
                    .as_ref()
                    .is_some_and(|(_, current)| !current.phase().terminal())
                {
                    if sender
                        .send(BackendEvent::ExecutionPreflight {
                            id,
                            result: Err("Another execution is still active. Finish or cancel its confirmation first.".to_owned()),
                        })
                        .is_err()
                    {
                        return;
                    }
                    continue;
                }
                execution = None;
                match InteractiveExecution::prepare(*task) {
                    Ok(controller) => {
                        let preflight = controller.preflight().clone();
                        execution = Some((id, controller));
                        if sender
                            .send(BackendEvent::ExecutionPreflight {
                                id,
                                result: Ok(preflight),
                            })
                            .is_err()
                        {
                            return;
                        }
                    }
                    Err(error) => {
                        if sender
                            .send(BackendEvent::ExecutionPreflight {
                                id,
                                result: Err(error.to_string()),
                            })
                            .is_err()
                        {
                            return;
                        }
                    }
                }
                continue;
            }
            AppServerCommand::InspectExecution { id } => {
                let result = execution
                    .as_mut()
                    .filter(|(execution_id, _)| *execution_id == id)
                    .ok_or_else(|| "Execution preflight is no longer active.".to_owned())
                    .and_then(|(_, controller)| {
                        controller
                            .inspect_existing_after_confirmation()
                            .map_err(|error| error.to_string())
                    });
                match result {
                    Ok(preflight) => {
                        if sender
                            .send(BackendEvent::ExecutionPreflight {
                                id,
                                result: Ok(preflight),
                            })
                            .is_err()
                        {
                            return;
                        }
                    }
                    Err(error) => {
                        if sender
                            .send(BackendEvent::ExecutionProgress {
                                id,
                                event: ControllerEvent::Status(error),
                            })
                            .is_err()
                        {
                            return;
                        }
                        if sender
                            .send(BackendEvent::ExecutionActionComplete { id })
                            .is_err()
                        {
                            return;
                        }
                    }
                }
                continue;
            }
            AppServerCommand::SubmitExecution { id } => {
                let result = execution
                    .as_mut()
                    .filter(|(execution_id, _)| *execution_id == id)
                    .ok_or_else(|| "Execution preflight is no longer active.".to_owned())
                    .and_then(|(_, controller)| {
                        controller.submit().map_err(|error| error.to_string())
                    });
                match result {
                    Ok(events) => {
                        if !send_controller_events(&sender, id, events) {
                            return;
                        }
                    }
                    Err(error) => {
                        if sender
                            .send(BackendEvent::ExecutionProgress {
                                id,
                                event: ControllerEvent::Status(error),
                            })
                            .is_err()
                        {
                            return;
                        }
                    }
                }
                if sender
                    .send(BackendEvent::ExecutionActionComplete { id })
                    .is_err()
                {
                    return;
                }
                continue;
            }
            AppServerCommand::DecideApproval {
                id,
                request_id,
                choice,
            } => {
                let result = execution
                    .as_mut()
                    .filter(|(execution_id, _)| *execution_id == id)
                    .ok_or_else(|| "Execution is no longer active.".to_owned())
                    .and_then(|(_, controller)| {
                        controller
                            .decide_approval(&request_id, choice)
                            .map_err(|error| error.to_string())
                    });
                if let Err(error) = result {
                    if sender
                        .send(BackendEvent::ExecutionProgress {
                            id,
                            event: ControllerEvent::Status(error),
                        })
                        .is_err()
                    {
                        return;
                    }
                }
                if sender
                    .send(BackendEvent::ExecutionActionComplete { id })
                    .is_err()
                {
                    return;
                }
                continue;
            }
            AppServerCommand::InterruptExecution { id } => {
                let result = execution
                    .as_mut()
                    .filter(|(execution_id, _)| *execution_id == id)
                    .ok_or_else(|| "Execution is no longer active.".to_owned())
                    .and_then(|(_, controller)| {
                        controller.interrupt().map_err(|error| error.to_string())
                    });
                match result {
                    Ok(events) => {
                        if !send_controller_events(&sender, id, events) {
                            return;
                        }
                    }
                    Err(error) => {
                        if sender
                            .send(BackendEvent::ExecutionProgress {
                                id,
                                event: ControllerEvent::Status(error),
                            })
                            .is_err()
                        {
                            return;
                        }
                    }
                }
                if sender
                    .send(BackendEvent::ExecutionActionComplete { id })
                    .is_err()
                {
                    return;
                }
                continue;
            }
            AppServerCommand::ReconcileExecution { id } => {
                let events = execution
                    .as_mut()
                    .filter(|(execution_id, _)| *execution_id == id)
                    .map(|(_, controller)| controller.reconcile())
                    .unwrap_or_else(|| {
                        vec![ControllerEvent::Status(
                            "Execution recovery is no longer available.".to_owned(),
                        )]
                    });
                if !send_controller_events(&sender, id, events) {
                    return;
                }
                if sender
                    .send(BackendEvent::ExecutionActionComplete { id })
                    .is_err()
                {
                    return;
                }
                continue;
            }
            AppServerCommand::CancelExecution { id } => {
                let can_cancel = execution
                    .as_ref()
                    .is_some_and(|(execution_id, controller)| {
                        *execution_id == id
                            && controller.phase()
                                == crate::codex_app_server::ExecutionPhase::AwaitingConfirmation
                            && controller.turn_id().is_none()
                    });
                if can_cancel {
                    execution = None;
                    if sender
                        .send(BackendEvent::ExecutionCancelled { id })
                        .is_err()
                    {
                        return;
                    }
                } else if sender
                    .send(BackendEvent::ExecutionProgress {
                        id,
                        event: ControllerEvent::Status(
                            "This execution cannot be cancelled safely from its current state."
                                .to_owned(),
                        ),
                    })
                    .is_err()
                {
                    return;
                }
                continue;
            }
            command => {
                let (event, failed) = match command {
                    AppServerCommand::Sessions { id } => {
                        let mut result = with_client(&mut client, connector, |client| {
                            client.list_sessions(&SessionListOptions {
                                archive_filter: crate::codex_app_server::ArchiveFilter::All,
                                active_only: false,
                                limit: None,
                            })
                        });
                        if let Ok(sessions) = result.as_mut() {
                            sessions.sort_by(|left, right| {
                                right
                                    .updated_at
                                    .cmp(&left.updated_at)
                                    .then_with(|| left.thread_id.cmp(&right.thread_id))
                            });
                        }
                        (BackendEvent::Sessions { id, result }, client.is_none())
                    }
                    AppServerCommand::SessionDetail { id, thread_id } => {
                        let result = with_client(&mut client, connector, |client| {
                            client.read_session(&thread_id)
                        });
                        (BackendEvent::SessionDetail { id, result }, client.is_none())
                    }
                    AppServerCommand::Projects { id } => {
                        let result =
                            with_client(&mut client, connector, CodexAppServer::list_projects);
                        (BackendEvent::Projects { id, result }, client.is_none())
                    }
                    AppServerCommand::Models { id } => {
                        let result =
                            with_client(&mut client, connector, CodexAppServer::list_models);
                        (BackendEvent::Models { id, result }, client.is_none())
                    }
                    AppServerCommand::Resume { id, thread_id } => {
                        let result = with_client(&mut client, connector, |client| {
                            client.resume_session(&thread_id)
                        })
                        .map(|session| {
                            format!(
                                "Thread {} loaded. No prompt was sent and no turn was started.",
                                session.thread_id
                            )
                        });
                        (
                            BackendEvent::Mutation {
                                id,
                                kind: MutationKind::Resume,
                                result,
                            },
                            client.is_none(),
                        )
                    }
                    AppServerCommand::Fork { id, thread_id } => {
                        let result = with_client(&mut client, connector, |client| {
                            client.fork_session(&thread_id)
                        })
                        .map(|fork| {
                            format!(
                                "Fork created as thread {}. No turn was started.",
                                fork.thread_id
                            )
                        });
                        (
                            BackendEvent::Mutation {
                                id,
                                kind: MutationKind::Fork,
                                result,
                            },
                            client.is_none(),
                        )
                    }
                    AppServerCommand::PrepareExecution { .. }
                    | AppServerCommand::InspectExecution { .. }
                    | AppServerCommand::SubmitExecution { .. }
                    | AppServerCommand::DecideApproval { .. }
                    | AppServerCommand::InterruptExecution { .. }
                    | AppServerCommand::ReconcileExecution { .. }
                    | AppServerCommand::CancelExecution { .. } => unreachable!(),
                };
                if failed {
                    client = None;
                }
                if shutdown.load(Ordering::Acquire) {
                    return;
                }
                if sender.send(event).is_err() {
                    return;
                }
            }
        }
    }
}

fn send_controller_events(
    sender: &SyncSender<BackendEvent>,
    id: u64,
    events: Vec<ControllerEvent>,
) -> bool {
    for event in events {
        if sender
            .send(BackendEvent::ExecutionProgress { id, event })
            .is_err()
        {
            return false;
        }
    }
    true
}

fn receive_command<T>(receiver: &Receiver<T>, shutdown: &AtomicBool) -> Option<T> {
    loop {
        if shutdown.load(Ordering::Acquire) {
            return None;
        }
        match receiver.recv_timeout(StdDuration::from_millis(100)) {
            Ok(command) => return Some(command),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return None,
        }
    }
}

fn with_client<T>(
    client: &mut Option<CodexAppServer>,
    connector: AppServerConnector,
    operation: impl FnOnce(&CodexAppServer) -> Result<T, CodexAppError>,
) -> Result<T, String> {
    if client.is_none() {
        *client = Some(connector().map_err(app_server_error)?);
    }
    let result = operation(client.as_ref().expect("client initialized")).map_err(app_server_error);
    if result.is_err() {
        *client = None;
    }
    result
}

#[cfg(test)]
fn connect_test_sessions_app_server() -> Result<CodexAppServer, CodexAppError> {
    CodexAppServer::connect_test_server("sessions")
}

#[cfg(test)]
fn connect_test_models_app_server() -> Result<CodexAppServer, CodexAppError> {
    CodexAppServer::connect_test_server("models")
}

fn app_server_error(error: CodexAppError) -> String {
    format!("Local Codex App Server is unavailable ({})", error.code())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn history_ranges_have_exclusive_upper_bound_and_expected_scope() {
        let (today_since, today_until) = history_bounds(HistoryScope::Today);
        assert!(today_since
            .as_deref()
            .is_some_and(|value| value.ends_with("Z")));
        assert!(today_until
            .as_deref()
            .is_some_and(|value| value.ends_with("Z")));
        let (week_since, week_until) = history_bounds(HistoryScope::Week);
        assert!(week_since.is_some());
        assert!(week_until.is_some());
        assert_eq!(history_bounds(HistoryScope::All), (None, None));
    }

    #[test]
    fn missing_database_is_reported_as_missing_evidence() {
        let missing = PathBuf::from("/a/path/that/does/not/exist/codex-meter.sqlite3");
        let data = load_overview(&missing).unwrap();
        assert_eq!(data.ledger_status, LedgerStatus::Missing);
        assert!(data.token_statistics.is_none());
        assert!(data.quota.is_none());
    }

    #[test]
    fn app_server_worker_uses_the_p033_fake_for_safe_session_actions() {
        let backend = Backend::new_test(
            std::env::temp_dir().join("codex-meter-tui-test.sqlite3"),
            TestAppServerMode::Sessions,
        )
        .unwrap();
        backend.request_sessions(1).unwrap();
        let BackendEvent::Sessions { id, result } = backend
            .result_receiver
            .recv_timeout(Duration::from_secs(3))
            .expect("fake App Server should return the bounded session list")
        else {
            panic!("expected a session-list result event");
        };
        assert_eq!(id, 1);
        let sessions = result.unwrap();
        assert_eq!(
            sessions
                .iter()
                .map(|session| session.thread_id.as_str())
                .collect::<Vec<_>>(),
            ["thread-archived", "thread-active", "thread-idle"]
        );
        assert!(sessions.iter().all(|session| {
            session.usage_link == crate::codex_app_server::UsageLinkAvailability::Unavailable
        }));
        let encoded = format!("{sessions:?}");
        assert!(!encoded.contains("SENSITIVE_PROMPT_SENTINEL"));
        assert!(!encoded.contains("/private/path/"));

        backend.request_projects(4).unwrap();
        let BackendEvent::Projects { id, result } = backend
            .result_receiver
            .recv_timeout(Duration::from_secs(3))
            .expect("fake App Server should return response-local project groups")
        else {
            panic!("expected a project result event");
        };
        assert_eq!(id, 4);
        let projects = result.unwrap();
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].project_alias, "project-001");
        assert_eq!(projects[0].session_count, 3);
        assert_eq!(
            projects[0].association,
            crate::codex_app_server::ProjectAssociationKind::CodexProjectId
        );

        backend
            .request_resume(2, "thread-active".to_owned())
            .unwrap();
        let BackendEvent::Mutation { id, kind, result } = backend
            .result_receiver
            .recv_timeout(Duration::from_secs(3))
            .expect("fake App Server should return a resume result")
        else {
            panic!("expected a resume result event");
        };
        assert_eq!(id, 2);
        assert_eq!(kind, MutationKind::Resume);
        assert!(result.unwrap().contains("No prompt was sent"));

        backend.request_fork(3, "thread-active".to_owned()).unwrap();
        let BackendEvent::Mutation { id, kind, result } = backend
            .result_receiver
            .recv_timeout(Duration::from_secs(3))
            .expect("fake App Server should return a fork result")
        else {
            panic!("expected a fork result event");
        };
        assert_eq!(id, 3);
        assert_eq!(kind, MutationKind::Fork);
        assert!(result.unwrap().contains("No turn was started"));
    }

    #[test]
    fn app_server_worker_uses_the_p033_live_model_catalog_service() {
        let backend = Backend::new_test(
            std::env::temp_dir().join("codex-meter-tui-model-test.sqlite3"),
            TestAppServerMode::Models,
        )
        .unwrap();
        backend.request_models(7).unwrap();
        let BackendEvent::Models { id, result } = backend
            .result_receiver
            .recv_timeout(Duration::from_secs(3))
            .expect("fake App Server should return the model catalog")
        else {
            panic!("expected a model catalog result event");
        };
        assert_eq!(id, 7);
        let catalog = result.unwrap();
        assert_eq!(catalog.models.len(), 2);
        assert!(catalog.models[0].is_default);
        assert_eq!(catalog.models[0].id, "model-visible");
        assert_eq!(catalog.models[1].id, "model-hidden");
    }

    #[test]
    fn backend_shutdown_joins_workers_with_a_request_queued() {
        let backend = Backend::new_test(
            std::env::temp_dir().join("codex-meter-tui-shutdown-test.sqlite3"),
            TestAppServerMode::Sessions,
        )
        .unwrap();
        backend.request_sessions(9).unwrap();
        drop(backend);
    }
}
