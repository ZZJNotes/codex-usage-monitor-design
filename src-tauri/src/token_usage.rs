use std::{
    collections::{BTreeMap, hash_map::DefaultHasher},
    fs::{self, File},
    hash::{Hash, Hasher},
    io::{BufRead, BufReader, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::Mutex,
};

use chrono::{DateTime, SecondsFormat, Utc};
use rusqlite::{OptionalExtension, params, params_from_iter, types::Value as SqlValue};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::database::Database;

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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseResult {
    pub events: Vec<TokenEvent>,
    pub malformed_lines: usize,
    pub unknown_events: usize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct ParserContext {
    session_id: Option<String>,
    model: Option<String>,
}

enum ParsedLine {
    Event(TokenEvent),
    Known,
    Unknown,
    Malformed,
}

pub fn parse_jsonl(input: &str, fallback_session_id: &str) -> ParseResult {
    let mut context = ParserContext::default();
    let mut result = ParseResult {
        events: Vec::new(),
        malformed_lines: 0,
        unknown_events: 0,
    };
    for line in input.lines() {
        apply_parsed_line(
            parse_line(line.as_bytes(), &mut context, fallback_session_id),
            &mut result,
        );
    }
    result
}

fn apply_parsed_line(line: ParsedLine, result: &mut ParseResult) {
    match line {
        ParsedLine::Event(event) => result.events.push(event),
        ParsedLine::Known => {}
        ParsedLine::Unknown => result.unknown_events += 1,
        ParsedLine::Malformed => result.malformed_lines += 1,
    }
}

fn parse_line(line: &[u8], context: &mut ParserContext, fallback: &str) -> ParsedLine {
    if line.iter().all(u8::is_ascii_whitespace) {
        return ParsedLine::Known;
    }
    let value: Value = match serde_json::from_slice(line) {
        Ok(value) => value,
        Err(_) => return ParsedLine::Malformed,
    };
    let event_type = value
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let payload = value.get("payload").unwrap_or(&Value::Null);
    match event_type {
        "session_meta" => {
            if let Some(id) = payload
                .get("id")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
            {
                context.session_id = Some(id.to_string());
            }
            ParsedLine::Known
        }
        "turn_context" => {
            if let Some(model) = payload
                .get("model")
                .and_then(Value::as_str)
                .filter(|model| !model.is_empty())
            {
                context.model = Some(model.to_string());
            }
            ParsedLine::Known
        }
        "event_msg" if payload.get("type").and_then(Value::as_str) == Some("token_count") => {
            let Some(usage) = payload
                .get("info")
                .and_then(|info| info.get("last_token_usage"))
                .filter(|usage| usage.is_object())
            else {
                // Cumulative usage is reconciliation metadata, not a second request event.
                return ParsedLine::Known;
            };
            let observed_at = value
                .get("timestamp")
                .and_then(Value::as_str)
                .and_then(|timestamp| DateTime::parse_from_rfc3339(timestamp).ok())
                .map(|timestamp| {
                    timestamp
                        .with_timezone(&Utc)
                        .to_rfc3339_opts(SecondsFormat::Millis, true)
                });
            let Some(observed_at) = observed_at else {
                return ParsedLine::Malformed;
            };
            let input_tokens = usage_u64(usage, "input_tokens");
            let output_tokens = usage_u64(usage, "output_tokens");
            let counts = TokenCounts {
                input_tokens,
                cached_input_tokens: usage_u64(usage, "cached_input_tokens"),
                cache_write_input_tokens: usage_u64(usage, "cache_write_input_tokens")
                    .max(usage_u64(usage, "cache_creation_input_tokens")),
                output_tokens,
                reasoning_output_tokens: usage_u64(usage, "reasoning_output_tokens"),
                // Upstream total fields are not trusted because subsets must not be added twice.
                total_tokens: input_tokens.saturating_add(output_tokens),
            };
            ParsedLine::Event(TokenEvent {
                session_id: context
                    .session_id
                    .clone()
                    .unwrap_or_else(|| fallback.to_string()),
                model: context
                    .model
                    .clone()
                    .unwrap_or_else(|| "unknown".to_string()),
                observed_at,
                counts,
            })
        }
        _ => ParsedLine::Unknown,
    }
}

fn usage_u64(usage: &Value, key: &str) -> u64 {
    usage.get(key).and_then(Value::as_u64).unwrap_or(0)
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenUsageFilters {
    pub start_at: Option<String>,
    pub end_at: Option<String>,
    pub model: Option<String>,
    pub session_id: Option<String>,
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
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenUsageData {
    pub totals: TokenCounts,
    pub models: Vec<ModelUsage>,
    pub sessions: Vec<SessionUsage>,
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
    last_data: Mutex<Option<TokenUsageData>>,
    last_scan_error: Mutex<Option<String>>,
}

impl TokenUsageService {
    pub fn new(database: Database, roots: Vec<PathBuf>) -> Self {
        Self {
            database,
            roots,
            last_data: Mutex::new(None),
            last_scan_error: Mutex::new(None),
        }
    }

    pub fn default_roots(database: Database) -> Self {
        Self::new(database, discover_default_roots())
    }

    pub fn scan(&self) -> Result<ImportReport, String> {
        let result = (|| {
            let mut report = ImportReport::default();
            for root in &self.roots {
                for path in jsonl_files(root)? {
                    report.scanned_files += 1;
                    let file_report = self.scan_file(&path)?;
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
        result
    }

    pub fn query(&self, filters: TokenUsageFilters) -> TokenUsageState {
        match query_usage(&self.database, &filters) {
            Ok(data) => {
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

    fn scan_file(&self, path: &Path) -> Result<ImportReport, String> {
        let source_key = opaque_hash(&path.to_string_lossy());
        let metadata =
            fs::metadata(path).map_err(|error| redacted_io_error("read session", error))?;
        let mut checkpoint = load_checkpoint(&self.database, &source_key)?.unwrap_or(Checkpoint {
            offset: 0,
            context: ParserContext::default(),
        });
        if metadata.len() < checkpoint.offset {
            checkpoint.offset = 0;
            checkpoint.context = ParserContext::default();
        }

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
                    if insert_event(&self.database, &source_key, &event)? {
                        report.imported_events += 1;
                    }
                }
                ParsedLine::Known => {}
                ParsedLine::Unknown => report.unknown_events += 1,
                ParsedLine::Malformed => report.malformed_lines += 1,
            }
            offset += read as u64;
        }
        checkpoint.offset = offset;
        save_checkpoint(&self.database, &source_key, metadata.len(), &checkpoint)?;
        Ok(report)
    }
}

fn discover_default_roots() -> Vec<PathBuf> {
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
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

fn event_key(event: &TokenEvent) -> String {
    opaque_hash(&format!(
        "{}|{}|{}|{}|{}|{}|{}|{}",
        event.session_id,
        event.observed_at,
        event.model,
        event.counts.input_tokens,
        event.counts.cached_input_tokens,
        event.counts.cache_write_input_tokens,
        event.counts.output_tokens,
        event.counts.reasoning_output_tokens,
    ))
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

fn insert_event(database: &Database, source_key: &str, event: &TokenEvent) -> Result<bool, String> {
    database.with_connection(|connection| {
        connection
            .execute(
                "INSERT OR IGNORE INTO token_usage_events
                   (event_key, source_key, observed_at_utc, session_id, model,
                    input_tokens, cached_input_tokens, cache_write_input_tokens,
                    output_tokens, reasoning_output_tokens, total_tokens)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                params![
                    event_key(event),
                    source_key,
                    event.observed_at,
                    event.session_id,
                    event.model,
                    to_i64(event.counts.input_tokens),
                    to_i64(event.counts.cached_input_tokens),
                    to_i64(event.counts.cache_write_input_tokens),
                    to_i64(event.counts.output_tokens),
                    to_i64(event.counts.reasoning_output_tokens),
                    to_i64(event.counts.total_tokens),
                ],
            )
            .map(|changed| changed == 1)
            .map_err(|error| error.to_string())
    })
}

fn to_i64(value: u64) -> i64 {
    value.min(i64::MAX as u64) as i64
}

fn query_usage(database: &Database, filters: &TokenUsageFilters) -> Result<TokenUsageData, String> {
    database.with_connection(|connection| {
        let mut sql = String::from(
            "SELECT observed_at_utc, session_id, model, input_tokens,
                    cached_input_tokens, cache_write_input_tokens, output_tokens,
                    reasoning_output_tokens, total_tokens
             FROM token_usage_events WHERE 1 = 1",
        );
        let mut values = Vec::<SqlValue>::new();
        for (column, value, operator) in [
            ("observed_at_utc", filters.start_at.as_ref(), ">="),
            ("observed_at_utc", filters.end_at.as_ref(), "<="),
            ("model", filters.model.as_ref(), "="),
            ("session_id", filters.session_id.as_ref(), "="),
        ] {
            if let Some(value) = value.filter(|value| !value.is_empty()) {
                sql.push_str(&format!(" AND {column} {operator} ?"));
                values.push(SqlValue::Text(value.clone()));
            }
        }
        sql.push_str(" ORDER BY observed_at_utc, event_key");
        let mut statement = connection
            .prepare(&sql)
            .map_err(|error| error.to_string())?;
        let rows = statement
            .query_map(params_from_iter(values), |row| {
                Ok(TokenEvent {
                    observed_at: row.get(0)?,
                    session_id: row.get(1)?,
                    model: row.get(2)?,
                    counts: TokenCounts {
                        input_tokens: row.get::<_, i64>(3)?.max(0) as u64,
                        cached_input_tokens: row.get::<_, i64>(4)?.max(0) as u64,
                        cache_write_input_tokens: row.get::<_, i64>(5)?.max(0) as u64,
                        output_tokens: row.get::<_, i64>(6)?.max(0) as u64,
                        reasoning_output_tokens: row.get::<_, i64>(7)?.max(0) as u64,
                        total_tokens: row.get::<_, i64>(8)?.max(0) as u64,
                    },
                })
            })
            .map_err(|error| error.to_string())?;

        let mut totals = TokenCounts::default();
        let mut models = BTreeMap::<String, TokenCounts>::new();
        let mut sessions = BTreeMap::<(String, String), SessionUsage>::new();
        let mut updated_at = None;
        for row in rows {
            let event = row.map_err(|error| error.to_string())?;
            totals.add_assign(&event.counts);
            models
                .entry(event.model.clone())
                .or_default()
                .add_assign(&event.counts);
            let session = sessions
                .entry((event.session_id.clone(), event.model.clone()))
                .or_insert_with(|| SessionUsage {
                    session_id: event.session_id.clone(),
                    model: event.model.clone(),
                    first_observed_at: event.observed_at.clone(),
                    last_observed_at: event.observed_at.clone(),
                    counts: TokenCounts::default(),
                });
            session.last_observed_at = event.observed_at.clone();
            session.counts.add_assign(&event.counts);
            updated_at = Some(event.observed_at);
        }
        Ok(TokenUsageData {
            totals,
            models: models
                .into_iter()
                .map(|(model, counts)| ModelUsage { model, counts })
                .collect(),
            sessions: sessions.into_values().collect(),
            updated_at: updated_at.unwrap_or_else(|| Utc::now().to_rfc3339()),
        })
    })
}

#[cfg(test)]
mod tests {
    use std::{fs::OpenOptions, io::Write};

    use tempfile::tempdir;

    use super::*;

    fn ready(state: TokenUsageState) -> TokenUsageData {
        match state {
            TokenUsageState::Ready { data } => data,
            TokenUsageState::Error { message, .. } => panic!("unexpected error: {message}"),
            TokenUsageState::Loading => panic!("unexpected loading state"),
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
}
