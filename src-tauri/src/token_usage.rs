use std::{
    collections::BTreeSet,
    fs::{self, File},
    io::{BufRead, BufReader, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, mpsc::Sender},
};

use chrono::{DateTime, SecondsFormat, Utc};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::database::Database;

#[path = "token_usage/parser.rs"]
mod parser;
pub use parser::{ParseResult, parse_jsonl};
use parser::{ParsedLine, ParserContext, parse_line};
#[path = "token_usage/repository.rs"]
mod repository;
use repository::{
    insert_event, query_usage, reassign_session, record_account_evidence,
    record_session_attribution,
};

pub const UNASSIGNED_ACCOUNT_FILTER: &str = "unassigned";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenAccount {
    pub account_key: String,
    pub display_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveAccountEvidence {
    pub account: TokenAccount,
    pub source: String,
    pub observed_at: String,
}

pub trait AccountEvidenceSource: Send + Sync {
    fn active_account(&self) -> Option<ActiveAccountEvidence>;

    fn known_accounts(&self) -> Vec<ActiveAccountEvidence> {
        self.active_account().into_iter().collect()
    }
}

struct NoAccountEvidence;

impl AccountEvidenceSource for NoAccountEvidence {
    fn active_account(&self) -> Option<ActiveAccountEvidence> {
        None
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenCounts {
    pub input_tokens: u64,
    pub cached_input_tokens: u64,
    pub cache_write_input_tokens: u64,
    pub output_tokens: u64,
    pub reasoning_output_tokens: u64,
    pub total_tokens: u64,
}

impl TokenCounts {
    fn add_assign(&mut self, other: &Self) {
        self.input_tokens = self.input_tokens.saturating_add(other.input_tokens);
        self.cached_input_tokens = self
            .cached_input_tokens
            .saturating_add(other.cached_input_tokens);
        self.cache_write_input_tokens = self
            .cache_write_input_tokens
            .saturating_add(other.cache_write_input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(other.output_tokens);
        self.reasoning_output_tokens = self
            .reasoning_output_tokens
            .saturating_add(other.reasoning_output_tokens);
        self.total_tokens = self.total_tokens.saturating_add(other.total_tokens);
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenEvent {
    pub session_id: String,
    pub model: String,
    pub observed_at: String,
    pub counts: TokenCounts,
    #[serde(skip)]
    pub(super) event_ordinal: u64,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenUsageFilters {
    pub start_at: Option<String>,
    pub end_at: Option<String>,
    pub model: Option<String>,
    pub session_id: Option<String>,
    pub account_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelUsage {
    pub model: String,
    pub counts: TokenCounts,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionUsage {
    pub session_id: String,
    pub model: String,
    pub first_observed_at: String,
    pub last_observed_at: String,
    pub counts: TokenCounts,
    pub assignment: SessionAttribution,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum AttributionSource {
    ActiveAccount,
    Unassigned,
    Manual,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionAttribution {
    pub account: Option<TokenAccount>,
    pub source: AttributionSource,
    pub assigned_at: String,
    pub evidence_source: Option<String>,
    pub evidence_observed_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenUsageData {
    pub totals: TokenCounts,
    pub models: Vec<ModelUsage>,
    pub sessions: Vec<SessionUsage>,
    pub accounts: Vec<TokenAccount>,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum TokenUsageState {
    Loading,
    Ready {
        data: TokenUsageData,
    },
    Error {
        message: String,
        last_data: Option<TokenUsageData>,
    },
    Stale {
        data: Option<TokenUsageData>,
        reason: TokenUsageStaleReason,
    },
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum TokenUsageStaleReason {
    Paused,
    Outdated,
}

impl TokenUsageState {
    pub fn paused(self) -> Self {
        match self {
            Self::Ready { data } => Self::Stale {
                data: Some(data),
                reason: TokenUsageStaleReason::Paused,
            },
            Self::Stale { data, .. } => Self::Stale {
                data,
                reason: TokenUsageStaleReason::Paused,
            },
            Self::Loading => Self::Stale {
                data: None,
                reason: TokenUsageStaleReason::Paused,
            },
            other => other,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImportReport {
    pub scanned_files: usize,
    pub imported_events: usize,
    pub malformed_lines: usize,
    pub unknown_events: usize,
}

#[derive(Clone)]
struct Checkpoint {
    offset: u64,
    context: ParserContext,
}

pub struct TokenUsageService {
    database: Database,
    roots: Vec<PathBuf>,
    account_evidence: Arc<dyn AccountEvidenceSource>,
    last_data: Mutex<Option<TokenUsageData>>,
    last_scan_error: Mutex<Option<String>>,
    last_scan_at: Mutex<Option<DateTime<Utc>>>,
}

impl TokenUsageService {
    pub fn new(database: Database, roots: Vec<PathBuf>) -> Self {
        Self::with_account_evidence(database, roots, Arc::new(NoAccountEvidence))
    }

    pub fn with_account_evidence(
        database: Database,
        roots: Vec<PathBuf>,
        account_evidence: Arc<dyn AccountEvidenceSource>,
    ) -> Self {
        Self {
            database,
            roots,
            account_evidence,
            last_data: Mutex::new(None),
            last_scan_error: Mutex::new(None),
            last_scan_at: Mutex::new(None),
        }
    }

    pub fn default_roots(database: Database) -> Self {
        Self::new(database, default_roots())
    }

    pub fn scan(&self) -> Result<ImportReport, String> {
        let result = (|| {
            let active_account = self.account_evidence.active_account();
            for evidence in self.account_evidence.known_accounts() {
                record_account_evidence(&self.database, &evidence)?;
            }
            let mut report = ImportReport::default();
            for root in &self.roots {
                for path in jsonl_files(root)? {
                    report.scanned_files += 1;
                    let file_report = self.scan_file(&path, active_account.as_ref())?;
                    report.imported_events += file_report.imported_events;
                    report.malformed_lines += file_report.malformed_lines;
                    report.unknown_events += file_report.unknown_events;
                }
            }
            Ok(report)
        })();
        *self
            .last_scan_error
            .lock()
            .expect("token error lock poisoned") = result.as_ref().err().cloned();
        if result.is_ok() {
            *self
                .last_scan_at
                .lock()
                .expect("token scan time lock poisoned") = Some(Utc::now());
        }
        result
    }

    pub fn query(&self, filters: TokenUsageFilters) -> TokenUsageState {
        match query_usage(&self.database, &filters) {
            Ok(data) => {
                let Some(scanned_at) = *self
                    .last_scan_at
                    .lock()
                    .expect("token scan time lock poisoned")
                else {
                    return TokenUsageState::Loading;
                };
                let mut data = data;
                data.updated_at = scanned_at.to_rfc3339_opts(SecondsFormat::Millis, true);
                *self.last_data.lock().expect("token data lock poisoned") = Some(data.clone());
                match self
                    .last_scan_error
                    .lock()
                    .expect("token error lock poisoned")
                    .clone()
                {
                    Some(message) => TokenUsageState::Error {
                        message,
                        last_data: Some(data),
                    },
                    None if (Utc::now() - scanned_at).num_seconds() > 45 => {
                        TokenUsageState::Stale {
                            data: Some(data),
                            reason: TokenUsageStaleReason::Outdated,
                        }
                    }
                    None => TokenUsageState::Ready { data },
                }
            }
            Err(message) => TokenUsageState::Error {
                message,
                last_data: self
                    .last_data
                    .lock()
                    .expect("token data lock poisoned")
                    .clone(),
            },
        }
    }

    pub fn reassign_session(
        &self,
        session_id: &str,
        account_key: Option<&str>,
        assigned_at: &str,
    ) -> Result<(), String> {
        if session_id.trim().is_empty() {
            return Err("session id is required".to_string());
        }
        reassign_session(&self.database, session_id, account_key, assigned_at)
    }

    fn scan_file(
        &self,
        path: &Path,
        active_account: Option<&ActiveAccountEvidence>,
    ) -> Result<ImportReport, String> {
        let source_key = opaque_hash(&path.to_string_lossy());
        let metadata =
            fs::metadata(path).map_err(|error| redacted_io_error("read session", error))?;
        let mut checkpoint = load_checkpoint(&self.database, &source_key)?.unwrap_or(Checkpoint {
            offset: 0,
            context: ParserContext::default(),
        });
        let identity = file_identity(&metadata);
        let anchor_matches = if checkpoint.offset == 0 {
            true
        } else if metadata.len() < checkpoint.offset || checkpoint.context.file_identity != identity
        {
            false
        } else {
            checkpoint.context.anchor_hash.as_deref()
                == Some(anchor_hash(path, checkpoint.offset)?.as_str())
        };
        if !anchor_matches {
            checkpoint.offset = 0;
            checkpoint.context = ParserContext::default();
        }
        checkpoint.context.file_identity = identity;

        let mut reader = BufReader::new(
            File::open(path).map_err(|error| redacted_io_error("open session", error))?,
        );
        reader
            .seek(SeekFrom::Start(checkpoint.offset))
            .map_err(|error| redacted_io_error("seek session", error))?;
        let mut report = ImportReport::default();
        let mut offset = checkpoint.offset;
        loop {
            let mut bytes = Vec::new();
            let read = reader
                .read_until(b'\n', &mut bytes)
                .map_err(|error| redacted_io_error("stream session", error))?;
            if read == 0 {
                break;
            }
            let complete_line = bytes.last() == Some(&b'\n');
            while matches!(bytes.last(), Some(b'\n' | b'\r')) {
                bytes.pop();
            }
            let previous_context = checkpoint.context.clone();
            let parsed = parse_line(&bytes, &mut checkpoint.context, &source_key);
            if !complete_line && matches!(parsed, ParsedLine::Malformed) {
                // A writer may still be appending this JSON value. Retry from the same offset.
                checkpoint.context = previous_context;
                break;
            }
            match parsed {
                ParsedLine::Event(event) => {
                    record_session_attribution(&self.database, &event.session_id, active_account)?;
                    if insert_event(&self.database, &source_key, &event)? {
                        report.imported_events += 1;
                    }
                }
                ParsedLine::SessionObserved(session_id) => {
                    record_session_attribution(&self.database, &session_id, active_account)?;
                }
                ParsedLine::Known => {}
                ParsedLine::Unknown => report.unknown_events += 1,
                ParsedLine::Malformed => report.malformed_lines += 1,
            }
            offset += read as u64;
        }
        checkpoint.offset = offset;
        checkpoint.context.anchor_hash = Some(anchor_hash(path, offset)?);
        save_checkpoint(&self.database, &source_key, metadata.len(), &checkpoint)?;
        Ok(report)
    }

    pub fn watcher(&self, sender: Sender<()>) -> Result<RecommendedWatcher, String> {
        let mut watcher =
            notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                if event.is_ok() {
                    let _ = sender.send(());
                }
            })
            .map_err(|_| "create session watcher failed".to_string())?;
        let mut watched = BTreeSet::new();
        for root in &self.roots {
            let target = if root.exists() {
                Some(root.as_path())
            } else {
                root.parent().filter(|parent| parent.exists())
            };
            if let Some(target) = target.filter(|target| watched.insert((*target).to_path_buf())) {
                watcher
                    .watch(target, RecursiveMode::Recursive)
                    .map_err(|_| "watch sessions failed".to_string())?;
            }
        }
        Ok(watcher)
    }
}

pub(crate) fn default_roots() -> Vec<PathBuf> {
    let codex_home = std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".codex")));
    codex_home
        .map(|root| vec![root.join("sessions"), root.join("archived_sessions")])
        .unwrap_or_default()
}

fn jsonl_files(root: &Path) -> Result<Vec<PathBuf>, String> {
    if !root.exists() {
        return Ok(Vec::new());
    }
    let mut files = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let entries = fs::read_dir(&directory)
            .map_err(|error| redacted_io_error("enumerate sessions", error))?;
        for entry in entries {
            let entry = entry.map_err(|error| redacted_io_error("enumerate sessions", error))?;
            let file_type = entry
                .file_type()
                .map_err(|error| redacted_io_error("inspect session", error))?;
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir() {
                pending.push(entry.path());
            } else if file_type.is_file()
                && entry.path().extension().and_then(|value| value.to_str()) == Some("jsonl")
            {
                files.push(entry.path());
            }
        }
    }
    files.sort();
    Ok(files)
}

fn redacted_io_error(action: &str, error: std::io::Error) -> String {
    format!("{action} failed: {}", error.kind())
}

fn opaque_hash(value: &str) -> String {
    hex_digest(value.as_bytes())
}

fn hex_digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn anchor_hash(path: &Path, offset: u64) -> Result<String, String> {
    if offset == 0 {
        return Ok(hex_digest(&[]));
    }
    const ANCHOR_SIZE: u64 = 512;
    let start = offset.saturating_sub(ANCHOR_SIZE);
    let mut file = File::open(path).map_err(|error| redacted_io_error("open session", error))?;
    file.seek(SeekFrom::Start(start))
        .map_err(|error| redacted_io_error("seek session", error))?;
    let mut bytes = vec![0; (offset - start) as usize];
    file.read_exact(&mut bytes)
        .map_err(|error| redacted_io_error("verify session", error))?;
    Ok(hex_digest(&bytes))
}

#[cfg(unix)]
fn file_identity(metadata: &fs::Metadata) -> Option<String> {
    use std::os::unix::fs::MetadataExt;
    Some(format!("{}:{}", metadata.dev(), metadata.ino()))
}

#[cfg(not(unix))]
fn file_identity(_metadata: &fs::Metadata) -> Option<String> {
    None
}

fn load_checkpoint(database: &Database, source_key: &str) -> Result<Option<Checkpoint>, String> {
    database.with_connection(|connection| {
        connection
            .query_row(
                "SELECT byte_offset, parser_context_json FROM token_import_checkpoints WHERE source_key = ?1",
                [source_key],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()
            .map_err(|error| error.to_string())?
            .map(|(offset, context)| {
                serde_json::from_str(&context)
                    .map(|context| Checkpoint { offset: offset.max(0) as u64, context })
                    .map_err(|error| error.to_string())
            })
            .transpose()
    })
}

fn save_checkpoint(
    database: &Database,
    source_key: &str,
    file_size: u64,
    checkpoint: &Checkpoint,
) -> Result<(), String> {
    let context = serde_json::to_string(&checkpoint.context).map_err(|error| error.to_string())?;
    database.with_connection(|connection| {
        connection
            .execute(
                "INSERT INTO token_import_checkpoints
                   (source_key, byte_offset, file_size, parser_context_json, updated_at_utc)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(source_key) DO UPDATE SET
                   byte_offset = excluded.byte_offset,
                   file_size = excluded.file_size,
                   parser_context_json = excluded.parser_context_json,
                   updated_at_utc = excluded.updated_at_utc",
                params![
                    source_key,
                    checkpoint.offset.min(i64::MAX as u64) as i64,
                    file_size.min(i64::MAX as u64) as i64,
                    context,
                    Utc::now().to_rfc3339(),
                ],
            )
            .map(|_| ())
            .map_err(|error| error.to_string())
    })
}

#[cfg(test)]
mod tests {
    use std::{
        fs::OpenOptions,
        io::{BufWriter, Write},
        sync::RwLock,
        time::{Duration, Instant},
    };

    use tempfile::tempdir;

    use super::*;

    struct FakeAccountEvidence(RwLock<Option<ActiveAccountEvidence>>);

    impl FakeAccountEvidence {
        fn new(evidence: Option<ActiveAccountEvidence>) -> Self {
            Self(RwLock::new(evidence))
        }

        fn set(&self, evidence: Option<ActiveAccountEvidence>) {
            *self.0.write().unwrap() = evidence;
        }
    }

    impl AccountEvidenceSource for FakeAccountEvidence {
        fn active_account(&self) -> Option<ActiveAccountEvidence> {
            self.0.read().unwrap().clone()
        }
    }

    struct RepresentativeAccountEvidence;

    impl AccountEvidenceSource for RepresentativeAccountEvidence {
        fn active_account(&self) -> Option<ActiveAccountEvidence> {
            Some(account(
                "sanitized-account-a",
                "Sanitized A",
                "2026-07-20T10:00:00Z",
            ))
        }

        fn known_accounts(&self) -> Vec<ActiveAccountEvidence> {
            vec![
                self.active_account().unwrap(),
                account("sanitized-account-b", "Sanitized B", "2026-07-20T10:00:00Z"),
            ]
        }
    }

    fn account(key: &str, name: &str, observed_at: &str) -> ActiveAccountEvidence {
        ActiveAccountEvidence {
            account: TokenAccount {
                account_key: key.to_string(),
                display_name: name.to_string(),
            },
            source: "sanitized-test-evidence".to_string(),
            observed_at: observed_at.to_string(),
        }
    }

    fn ready(state: TokenUsageState) -> TokenUsageData {
        match state {
            TokenUsageState::Ready { data } => data,
            TokenUsageState::Error { message, .. } => panic!("unexpected error: {message}"),
            TokenUsageState::Loading => panic!("unexpected loading state"),
            TokenUsageState::Stale {
                data: Some(data), ..
            } => data,
            TokenUsageState::Stale { data: None, .. } => panic!("unexpected empty stale state"),
        }
    }

    #[test]
    fn sanitized_jsonl_exposes_only_numeric_usage_and_minimal_identifiers() {
        let parsed = parse_jsonl(
            include_str!("../fixtures/token/active-session.jsonl"),
            "opaque-source",
        );

        assert_eq!(parsed.events.len(), 2);
        assert_eq!(parsed.malformed_lines, 1);
        assert_eq!(parsed.unknown_events, 2);
        assert_eq!(parsed.events[0].session_id, "sanitized-session-01");
        assert_eq!(parsed.events[0].model, "gpt-5.6");
        assert_eq!(parsed.events[0].counts.total_tokens, 150);
        assert_eq!(parsed.events[0].counts.cached_input_tokens, 40);
        assert_eq!(parsed.events[0].counts.cache_write_input_tokens, 12);
        assert_eq!(parsed.events[0].counts.reasoning_output_tokens, 10);
        let serialized = serde_json::to_string(&parsed.events).unwrap();
        assert!(!serialized.contains("PRIVATE_FIXTURE_SENTINEL"));
        assert!(!serialized.contains("never-store"));
    }

    #[test]
    fn service_imports_active_and_archive_once_and_filters_by_time_model_and_session() {
        let directory = tempdir().unwrap();
        let active = directory.path().join("sessions");
        let archive = directory.path().join("archived_sessions");
        fs::create_dir_all(&active).unwrap();
        fs::create_dir_all(&archive).unwrap();
        fs::write(
            active.join("active.jsonl"),
            include_str!("../fixtures/token/active-session.jsonl"),
        )
        .unwrap();
        fs::write(
            archive.join("archive.jsonl"),
            include_str!("../fixtures/token/archived-session.jsonl"),
        )
        .unwrap();
        let service = TokenUsageService::new(Database::in_memory().unwrap(), vec![active, archive]);

        let first = service.scan().unwrap();
        let repeated = service.scan().unwrap();
        assert_eq!(first.imported_events, 3);
        assert_eq!(first.malformed_lines, 1);
        assert_eq!(repeated.imported_events, 0);
        let all = ready(service.query(TokenUsageFilters::default()));
        assert_eq!(all.totals.input_tokens, 190);
        assert_eq!(all.totals.output_tokens, 90);
        assert_eq!(all.totals.total_tokens, 280);
        assert_eq!(all.models.len(), 2);
        assert_eq!(all.sessions.len(), 2);

        let filtered = ready(service.query(TokenUsageFilters {
            start_at: Some("2026-07-20T00:00:00Z".into()),
            model: Some("gpt-5.6".into()),
            session_id: Some("sanitized-session-01".into()),
            ..TokenUsageFilters::default()
        }));
        assert_eq!(filtered.totals.total_tokens, 180);
        assert_eq!(filtered.sessions.len(), 1);
        assert!(matches!(
            service.query(TokenUsageFilters::default()).paused(),
            TokenUsageState::Stale {
                reason: TokenUsageStaleReason::Paused,
                ..
            }
        ));
    }

    #[test]
    fn common_history_queries_meet_the_release_target_on_representative_sanitized_history() {
        let directory = tempdir().unwrap();
        let active = directory.path().join("sessions");
        fs::create_dir_all(&active).unwrap();
        let mut fixture =
            BufWriter::new(fs::File::create(active.join("release-performance.jsonl")).unwrap());
        for session in 0..100 {
            writeln!(
                fixture,
                r#"{{"timestamp":"2026-07-20T10:00:00Z","type":"session_meta","payload":{{"id":"sanitized-performance-session-{session:03}"}}}}"#
            )
            .unwrap();
            writeln!(
                fixture,
                r#"{{"timestamp":"2026-07-20T10:00:00Z","type":"turn_context","payload":{{"model":"gpt-5.{}"}}}}"#,
                4 + session % 4,
            )
            .unwrap();
            for event in 0..100 {
                let second = session * 100 + event;
                writeln!(
                    fixture,
                    r#"{{"timestamp":"2026-07-20T10:{:02}:{:02}Z","type":"event_msg","payload":{{"type":"token_count","info":{{"last_token_usage":{{"input_tokens":2,"cached_input_tokens":1,"output_tokens":1,"reasoning_output_tokens":1}}}}}}}}"#,
                    (second / 60) % 60,
                    second % 60,
                )
                .unwrap();
            }
        }
        fixture.flush().unwrap();

        let service = TokenUsageService::with_account_evidence(
            Database::open(&directory.path().join("release-performance.sqlite3")).unwrap(),
            vec![active],
            Arc::new(RepresentativeAccountEvidence),
        );
        assert_eq!(service.scan().unwrap().imported_events, 10_000);
        for session in 50..100 {
            service
                .reassign_session(
                    &format!("sanitized-performance-session-{session:03}"),
                    Some("sanitized-account-b"),
                    "2026-07-20T12:00:00Z",
                )
                .unwrap();
        }

        let cases = [
            ("all", TokenUsageFilters::default(), 30_000),
            (
                "time",
                TokenUsageFilters {
                    start_at: Some("2026-07-20T10:30:00Z".to_string()),
                    end_at: Some("2026-07-20T10:39:59Z".to_string()),
                    ..TokenUsageFilters::default()
                },
                5_400,
            ),
            (
                "model",
                TokenUsageFilters {
                    model: Some("gpt-5.6".to_string()),
                    ..TokenUsageFilters::default()
                },
                7_500,
            ),
            (
                "session",
                TokenUsageFilters {
                    session_id: Some("sanitized-performance-session-042".to_string()),
                    ..TokenUsageFilters::default()
                },
                300,
            ),
            (
                "account",
                TokenUsageFilters {
                    account_key: Some("sanitized-account-b".to_string()),
                    ..TokenUsageFilters::default()
                },
                15_000,
            ),
        ];

        for (name, filters, expected_total) in cases {
            let mut samples = Vec::new();
            for _ in 0..5 {
                let started = Instant::now();
                let data = ready(service.query(filters.clone()));
                let elapsed = started.elapsed();
                assert_eq!(data.totals.total_tokens, expected_total, "{name} total");
                samples.push(elapsed);
            }
            samples.sort();
            println!(
                "release query probe: storage=disk events=10000 accounts=2 sessions=100 models=4 runs=5 query={name} min_ms={} median_ms={} max_ms={}",
                samples[0].as_millis(),
                samples[2].as_millis(),
                samples[4].as_millis(),
            );
            assert!(
                samples[4] < Duration::from_millis(500),
                "{name} history query took {:?}",
                samples[4]
            );
        }
    }

    #[test]
    fn service_ingests_append_and_recovers_from_truncation_without_duplicates() {
        let directory = tempdir().unwrap();
        let active = directory.path().join("sessions");
        fs::create_dir_all(&active).unwrap();
        let path = active.join("active.jsonl");
        let prefix = concat!(
            "{\"timestamp\":\"2026-07-21T10:00:00Z\",\"type\":\"session_meta\",\"payload\":{\"id\":\"append-session\"}}\n",
            "{\"timestamp\":\"2026-07-21T10:00:01Z\",\"type\":\"turn_context\",\"payload\":{\"model\":\"gpt-5.6\"}}\n",
            "{\"timestamp\":\"2026-07-21T10:00:02Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"last_token_usage\":{\"input_tokens\":4,\"output_tokens\":6}}}}\n"
        );
        fs::write(&path, prefix).unwrap();
        let service = TokenUsageService::new(Database::in_memory().unwrap(), vec![active]);
        assert_eq!(service.scan().unwrap().imported_events, 1);

        OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"{\"timestamp\":\"2026-07-21T10:00:03Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"last_token_usage\":{\"input_tokens\":10,\"cached_input_tokens\":3,\"output_tokens\":5,\"reasoning_output_tokens\":2}}}}\n")
            .unwrap();
        assert_eq!(service.scan().unwrap().imported_events, 1);
        assert_eq!(
            ready(service.query(TokenUsageFilters::default()))
                .totals
                .total_tokens,
            25
        );

        fs::write(&path, prefix).unwrap();
        assert_eq!(service.scan().unwrap().imported_events, 0);
        assert_eq!(
            ready(service.query(TokenUsageFilters::default()))
                .totals
                .total_tokens,
            25
        );
    }

    #[test]
    fn moved_archive_and_incomplete_tail_remain_idempotent_and_private() {
        let directory = tempdir().unwrap();
        let active = directory.path().join("sessions");
        let archive = directory.path().join("archived_sessions");
        fs::create_dir_all(&active).unwrap();
        fs::create_dir_all(&archive).unwrap();
        let active_path = active.join("private-name-never-store.jsonl");
        let fixture = include_str!("../fixtures/token/active-session.jsonl");
        fs::write(&active_path, fixture).unwrap();
        let database = Database::in_memory().unwrap();
        let service = TokenUsageService::new(database.clone(), vec![active, archive.clone()]);

        assert_eq!(service.scan().unwrap().imported_events, 2);
        fs::write(archive.join("moved.jsonl"), fixture).unwrap();
        assert_eq!(service.scan().unwrap().imported_events, 0);

        let partial_path = archive.join("partial.jsonl");
        fs::write(
            &partial_path,
            "{\"timestamp\":\"2026-07-22T00:00:00Z\",\"type\":\"session_meta\",\"payload\":{\"id\":\"partial-session\"}}\n{\"timestamp\":\"2026-07-22T00:00:01Z\",\"type\":\"event_msg\"",
        )
        .unwrap();
        assert_eq!(service.scan().unwrap().imported_events, 0);
        OpenOptions::new()
            .append(true)
            .open(&partial_path)
            .unwrap()
            .write_all(b",\"payload\":{\"type\":\"token_count\",\"info\":{\"last_token_usage\":{\"input_tokens\":2,\"output_tokens\":3}}}}\n")
            .unwrap();
        assert_eq!(service.scan().unwrap().imported_events, 1);
        assert_eq!(
            ready(service.query(TokenUsageFilters::default()))
                .totals
                .total_tokens,
            185
        );

        database
            .with_connection(|connection| {
                let persisted: String = connection
                    .query_row(
                        "SELECT group_concat(source_key || parser_context_json, '|') FROM token_import_checkpoints",
                        [],
                        |row| row.get(0),
                    )
                    .map_err(|error| error.to_string())?;
                assert!(!persisted.contains("private-name-never-store"));
                assert!(!persisted.contains("PRIVATE_FIXTURE_SENTINEL"));
                assert!(!persisted.contains("REDACTED"));
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn identical_usage_records_in_the_same_millisecond_are_distinct_requests() {
        let directory = tempdir().unwrap();
        let active = directory.path().join("sessions");
        fs::create_dir_all(&active).unwrap();
        fs::write(
            active.join("same-time.jsonl"),
            concat!(
                "{\"timestamp\":\"2026-07-23T00:00:00Z\",\"type\":\"session_meta\",\"payload\":{\"id\":\"same-time-session\"}}\n",
                "{\"timestamp\":\"2026-07-23T00:00:01.123456Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"last_token_usage\":{\"input_tokens\":4,\"output_tokens\":6}}}}\n",
                "{\"timestamp\":\"2026-07-23T00:00:01.123789Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"last_token_usage\":{\"input_tokens\":4,\"output_tokens\":6}}}}\n"
            ),
        )
        .unwrap();
        let service = TokenUsageService::new(Database::in_memory().unwrap(), vec![active]);

        assert_eq!(service.scan().unwrap().imported_events, 2);
        assert_eq!(
            ready(service.query(TokenUsageFilters::default()))
                .totals
                .total_tokens,
            20
        );
        assert_eq!(service.scan().unwrap().imported_events, 0);
    }

    #[test]
    fn paused_service_reports_text_ready_state_without_reading_session_files() {
        let directory = tempdir().unwrap();
        let service = TokenUsageService::new(
            Database::in_memory().unwrap(),
            vec![directory.path().join("sessions")],
        );

        assert!(matches!(
            service.query(TokenUsageFilters::default()).paused(),
            TokenUsageState::Stale {
                data: None,
                reason: TokenUsageStaleReason::Paused,
            }
        ));
    }

    #[test]
    fn first_observation_freezes_only_explicit_active_account_evidence() {
        let directory = tempdir().unwrap();
        let active = directory.path().join("sessions");
        fs::create_dir_all(&active).unwrap();
        fs::write(
            active.join("first.jsonl"),
            concat!(
                "{\"timestamp\":\"2026-07-24T00:00:00Z\",\"type\":\"session_meta\",\"payload\":{\"id\":\"explicit-session\"}}\n",
                "{\"timestamp\":\"2026-07-24T00:00:01Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"last_token_usage\":{\"input_tokens\":7,\"output_tokens\":3}}}}\n"
            ),
        )
        .unwrap();
        let evidence = Arc::new(FakeAccountEvidence::new(Some(account(
            "acct_a",
            "Account A",
            "2026-07-24T00:00:00Z",
        ))));
        let service = TokenUsageService::with_account_evidence(
            Database::in_memory().unwrap(),
            vec![active.clone()],
            evidence.clone(),
        );

        service.scan().unwrap();
        evidence.set(Some(account("acct_b", "Account B", "2026-07-24T00:01:00Z")));
        fs::write(
            active.join("second.jsonl"),
            concat!(
                "{\"timestamp\":\"2026-07-24T00:01:00Z\",\"type\":\"session_meta\",\"payload\":{\"id\":\"later-session\"}}\n",
                "{\"timestamp\":\"2026-07-24T00:01:01Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"last_token_usage\":{\"input_tokens\":5,\"output_tokens\":5}}}}\n"
            ),
        )
        .unwrap();
        service.scan().unwrap();

        let all = ready(service.query(TokenUsageFilters::default()));
        let explicit = all
            .sessions
            .iter()
            .find(|session| session.session_id == "explicit-session")
            .unwrap();
        let later = all
            .sessions
            .iter()
            .find(|session| session.session_id == "later-session")
            .unwrap();
        assert_eq!(
            explicit.assignment.account.as_ref().unwrap().account_key,
            "acct_a"
        );
        assert_eq!(explicit.assignment.source, AttributionSource::ActiveAccount);
        assert_eq!(
            explicit.assignment.evidence_observed_at.as_deref(),
            Some("2026-07-24T00:00:00Z")
        );
        assert_eq!(
            later.assignment.account.as_ref().unwrap().account_key,
            "acct_b"
        );
    }

    #[test]
    fn session_metadata_observation_freezes_before_the_first_token_event() {
        let directory = tempdir().unwrap();
        let active = directory.path().join("sessions");
        fs::create_dir_all(&active).unwrap();
        let path = active.join("metadata-first.jsonl");
        fs::write(
            &path,
            "{\"timestamp\":\"2026-07-24T02:00:00Z\",\"type\":\"session_meta\",\"payload\":{\"id\":\"metadata-first-session\"}}\n",
        )
        .unwrap();
        let evidence = Arc::new(FakeAccountEvidence::new(Some(account(
            "acct_a",
            "Account A",
            "2026-07-24T02:00:00Z",
        ))));
        let service = TokenUsageService::with_account_evidence(
            Database::in_memory().unwrap(),
            vec![active],
            evidence.clone(),
        );
        service.scan().unwrap();

        evidence.set(Some(account("acct_b", "Account B", "2026-07-24T02:01:00Z")));
        OpenOptions::new()
            .append(true)
            .open(path)
            .unwrap()
            .write_all(b"{\"timestamp\":\"2026-07-24T02:01:01Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"last_token_usage\":{\"input_tokens\":2,\"output_tokens\":3}}}}\n")
            .unwrap();
        service.scan().unwrap();

        let data = ready(service.query(TokenUsageFilters::default()));
        assert_eq!(
            data.sessions[0]
                .assignment
                .account
                .as_ref()
                .unwrap()
                .account_key,
            "acct_a"
        );
    }

    #[test]
    fn absent_evidence_is_explicitly_unassigned_and_never_guessed_later() {
        let directory = tempdir().unwrap();
        let active = directory.path().join("sessions");
        fs::create_dir_all(&active).unwrap();
        let path = active.join("unassigned.jsonl");
        fs::write(
            &path,
            concat!(
                "{\"timestamp\":\"2026-07-25T00:00:00Z\",\"type\":\"session_meta\",\"payload\":{\"id\":\"unassigned-session\"}}\n",
                "{\"timestamp\":\"2026-07-25T00:00:01Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"last_token_usage\":{\"input_tokens\":8,\"output_tokens\":2}}}}\n"
            ),
        )
        .unwrap();
        let evidence = Arc::new(FakeAccountEvidence::new(None));
        let service = TokenUsageService::with_account_evidence(
            Database::in_memory().unwrap(),
            vec![active],
            evidence.clone(),
        );

        service.scan().unwrap();
        evidence.set(Some(account(
            "acct_after_quota_change",
            "Later Account",
            "2026-07-25T00:02:00Z",
        )));
        service.scan().unwrap();

        let data = ready(service.query(TokenUsageFilters {
            account_key: Some(UNASSIGNED_ACCOUNT_FILTER.to_string()),
            ..TokenUsageFilters::default()
        }));
        assert_eq!(data.totals.total_tokens, 10);
        assert!(data.sessions[0].assignment.account.is_none());
        assert_eq!(
            data.sessions[0].assignment.source,
            AttributionSource::Unassigned
        );
    }

    #[test]
    fn manual_reassignment_is_audited_and_changes_account_filtered_totals() {
        let directory = tempdir().unwrap();
        let active = directory.path().join("sessions");
        fs::create_dir_all(&active).unwrap();
        fs::write(
            active.join("manual.jsonl"),
            concat!(
                "{\"timestamp\":\"2026-07-25T01:00:00Z\",\"type\":\"session_meta\",\"payload\":{\"id\":\"manual-session\"}}\n",
                "{\"timestamp\":\"2026-07-25T01:00:01Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"last_token_usage\":{\"input_tokens\":12,\"output_tokens\":8}}}}\n"
            ),
        )
        .unwrap();
        let evidence = Arc::new(FakeAccountEvidence::new(Some(account(
            "acct_manual",
            "Manual Account",
            "2026-07-25T01:00:00Z",
        ))));
        let service = TokenUsageService::with_account_evidence(
            Database::in_memory().unwrap(),
            vec![active],
            evidence,
        );
        service.scan().unwrap();

        service
            .reassign_session("manual-session", None, "2026-07-25T02:00:00Z")
            .unwrap();
        let account_data = ready(service.query(TokenUsageFilters {
            account_key: Some("acct_manual".to_string()),
            ..TokenUsageFilters::default()
        }));
        assert_eq!(account_data.totals.total_tokens, 0);
        let unassigned = ready(service.query(TokenUsageFilters {
            account_key: Some(UNASSIGNED_ACCOUNT_FILTER.to_string()),
            ..TokenUsageFilters::default()
        }));
        assert_eq!(unassigned.totals.total_tokens, 20);
        assert_eq!(
            unassigned.sessions[0].assignment.source,
            AttributionSource::Manual
        );
        assert_eq!(
            unassigned.sessions[0].assignment.assigned_at,
            "2026-07-25T02:00:00Z"
        );

        service
            .reassign_session(
                "manual-session",
                Some("acct_manual"),
                "2026-07-25T03:00:00Z",
            )
            .unwrap();
        let reassigned = ready(service.query(TokenUsageFilters {
            account_key: Some("acct_manual".to_string()),
            ..TokenUsageFilters::default()
        }));
        assert_eq!(reassigned.totals.total_tokens, 20);
        assert_eq!(
            reassigned.sessions[0].assignment.source,
            AttributionSource::Manual
        );
        assert_eq!(
            reassigned.sessions[0].assignment.assigned_at,
            "2026-07-25T03:00:00Z"
        );
        assert!(
            service
                .reassign_session(
                    "manual-session",
                    Some("unknown-account"),
                    "2026-07-25T04:00:00Z",
                )
                .is_err()
        );
    }

    #[test]
    fn existing_token_history_migrates_to_explicit_unassigned_ownership() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("legacy.sqlite3");
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE token_usage_events (
                   event_key TEXT PRIMARY KEY, source_key TEXT NOT NULL,
                   observed_at_utc TEXT NOT NULL, session_id TEXT NOT NULL, model TEXT NOT NULL,
                   input_tokens INTEGER NOT NULL, cached_input_tokens INTEGER NOT NULL,
                   cache_write_input_tokens INTEGER NOT NULL, output_tokens INTEGER NOT NULL,
                   reasoning_output_tokens INTEGER NOT NULL, total_tokens INTEGER NOT NULL
                 );
                 INSERT INTO token_usage_events VALUES
                   ('event', 'opaque', '2026-07-19T00:00:00Z', 'legacy-session', 'gpt-5.6',
                    6, 0, 0, 4, 0, 10);",
            )
            .unwrap();
        drop(connection);
        let service = TokenUsageService::new(Database::open(&path).unwrap(), Vec::new());
        service.scan().unwrap();

        let unassigned = ready(service.query(TokenUsageFilters {
            account_key: Some(UNASSIGNED_ACCOUNT_FILTER.to_string()),
            ..TokenUsageFilters::default()
        }));

        assert_eq!(unassigned.totals.total_tokens, 10);
        assert_eq!(
            unassigned.sessions[0].assignment.source,
            AttributionSource::Unassigned
        );
    }
}
