use std::{fs::OpenOptions, io::Write, path::Path};

use chrono::{DateTime, Utc};
use rusqlite::{Transaction, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{database::Database, lifecycle::RetentionPeriod, quota::QuotaSnapshot};

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExportFormat {
    Json,
    Csv,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportArtifact {
    pub filename: String,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportReceipt {
    pub filename: String,
    pub destination: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum CredentialDeletionStatus {
    Available,
    Unavailable { reason: String },
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SafeExport {
    schema_version: u8,
    generated_at: DateTime<Utc>,
    quota_snapshots: Vec<QuotaExportRow>,
    token_usage: Vec<TokenExportRow>,
    system_health: Vec<SystemHealthExportRow>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct QuotaExportRow {
    observed_at: DateTime<Utc>,
    account_id: String,
    account_display_name: String,
    plan_type: String,
    window_name: String,
    remaining_percent: u8,
    resets_at: Option<DateTime<Utc>>,
    window_duration_minutes: Option<u64>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TokenExportRow {
    observed_at: String,
    account_id: Option<String>,
    session_id: String,
    model: String,
    input_tokens: u64,
    cached_input_tokens: u64,
    cache_write_input_tokens: u64,
    output_tokens: u64,
    reasoning_output_tokens: u64,
    total_tokens: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SystemHealthExportRow {
    bucket_kind: String,
    observed_at: String,
    sample_count: u64,
    cpu_percent_avg: f64,
    memory_used_bytes_avg: f64,
    disk_available_bytes_last: u64,
    network_down_bps_avg: f64,
    network_up_bps_avg: f64,
    battery_percent_last: Option<f64>,
    uptime_seconds_last: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryCleanupResult {
    pub quota_snapshots_deleted: usize,
    pub token_events_deleted: usize,
    pub system_aggregates_deleted: usize,
    pub session_attributions_deleted: usize,
    pub account_metadata_deleted: usize,
}

#[derive(Clone)]
pub struct DataGovernanceService {
    database: Database,
}

impl DataGovernanceService {
    pub fn new(database: Database) -> Self {
        Self { database }
    }

    pub fn database(&self) -> &Database {
        &self.database
    }

    pub fn cleanup_retention(
        &self,
        retention_days: u32,
        now: DateTime<Utc>,
    ) -> Result<HistoryCleanupResult, String> {
        let cutoff = RetentionPeriod::try_from(retention_days)?.cutoff(now)?;
        self.cleanup_history(Some(cutoff.to_rfc3339()))
    }

    pub fn clear_history(&self) -> Result<HistoryCleanupResult, String> {
        self.cleanup_history(None)
    }

    fn cleanup_history(&self, cutoff: Option<String>) -> Result<HistoryCleanupResult, String> {
        self.database.with_connection(|connection| {
            let transaction = connection
                .transaction()
                .map_err(|error| error.to_string())?;
            let quota_snapshots_deleted = delete_time_scoped(
                &transaction,
                "quota_snapshots",
                "observed_at_utc",
                cutoff.as_deref(),
            )?;
            let token_events_deleted = delete_time_scoped(
                &transaction,
                "token_usage_events",
                "observed_at_utc",
                cutoff.as_deref(),
            )?;
            let system_aggregates_deleted = delete_time_scoped(
                &transaction,
                "system_health_aggregates",
                "bucket_start_utc",
                cutoff.as_deref(),
            )?;
            let session_attributions_deleted = transaction
                .execute(
                    "DELETE FROM token_session_attributions WHERE NOT EXISTS (SELECT 1 FROM token_usage_events event WHERE event.session_id = token_session_attributions.session_id)",
                    [],
                )
                .map_err(|error| error.to_string())?;
            let account_metadata_deleted = transaction
                .execute(
                    "DELETE FROM token_accounts WHERE NOT EXISTS (SELECT 1 FROM token_session_attributions attribution WHERE attribution.account_key = token_accounts.account_key)",
                    [],
                )
                .map_err(|error| error.to_string())?;
            transaction.commit().map_err(|error| error.to_string())?;
            Ok(HistoryCleanupResult {
                quota_snapshots_deleted,
                token_events_deleted,
                system_aggregates_deleted,
                session_attributions_deleted,
                account_metadata_deleted,
            })
        })
    }

    pub fn delete_account_history(
        &self,
        account_key: &str,
    ) -> Result<HistoryCleanupResult, String> {
        if account_key.trim().is_empty() {
            return Err("account key is required".to_string());
        }
        self.database.with_connection(|connection| {
            let transaction = connection.transaction().map_err(|error| error.to_string())?;
            let token_events_deleted = transaction
                .execute(
                    "DELETE FROM token_usage_events WHERE session_id IN (SELECT session_id FROM token_session_attributions WHERE account_key = ?1)",
                    params![account_key],
                )
                .map_err(|error| error.to_string())?;
            let session_attributions_deleted = transaction
                .execute(
                    "DELETE FROM token_session_attributions WHERE account_key = ?1",
                    params![account_key],
                )
                .map_err(|error| error.to_string())?;
            let account_metadata_deleted = transaction
                .execute("DELETE FROM token_accounts WHERE account_key = ?1", params![account_key])
                .map_err(|error| error.to_string())?;
            let quota_snapshots_deleted = transaction
                .execute(
                    "DELETE FROM quota_snapshots WHERE account_key = ?1 OR json_extract(snapshot_json, '$.account.id') = ?1",
                    params![account_key],
                )
                .map_err(|error| error.to_string())?;
            transaction.commit().map_err(|error| error.to_string())?;
            Ok(HistoryCleanupResult {
                quota_snapshots_deleted,
                token_events_deleted,
                system_aggregates_deleted: 0,
                session_attributions_deleted,
                account_metadata_deleted,
            })
        })
    }

    pub fn export(
        &self,
        format: ExportFormat,
        generated_at: DateTime<Utc>,
    ) -> Result<ExportArtifact, String> {
        let export = self.load_safe_export(generated_at)?;
        let date = generated_at.format("%Y-%m-%d");
        match format {
            ExportFormat::Json => Ok(ExportArtifact {
                filename: format!("codex-usage-{date}.json"),
                content: serde_json::to_string_pretty(&export)
                    .map_err(|error| error.to_string())?,
            }),
            ExportFormat::Csv => Ok(ExportArtifact {
                filename: format!("codex-usage-{date}.csv"),
                content: safe_export_csv(&export),
            }),
        }
    }

    pub fn export_to_directory(
        &self,
        directory: &Path,
        display_directory: &str,
        format: ExportFormat,
        generated_at: DateTime<Utc>,
    ) -> Result<ExportReceipt, String> {
        let artifact = self.export(format, generated_at)?;
        let (stem, extension) = artifact
            .filename
            .rsplit_once('.')
            .ok_or_else(|| "export filename is invalid".to_string())?;
        for suffix in 0..1_000 {
            let filename = if suffix == 0 {
                artifact.filename.clone()
            } else {
                format!("{stem}-{suffix}.{extension}")
            };
            match OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(directory.join(&filename))
            {
                Ok(mut file) => {
                    file.write_all(artifact.content.as_bytes())
                        .map_err(|_| "write export failed".to_string())?;
                    return Ok(ExportReceipt {
                        destination: format!("{display_directory}/{filename}"),
                        filename,
                    });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(_) => return Err("create export failed".to_string()),
            }
        }
        Err("too many exports already exist for this date".to_string())
    }

    pub fn credential_deletion_status(&self) -> CredentialDeletionStatus {
        CredentialDeletionStatus::Available
    }

    pub fn request_credential_deletion(&self, account_key: &str) -> Result<(), String> {
        if account_key.trim().is_empty() {
            return Err("account key is required".to_string());
        }
        // Deletion of Keychain secrets is owned by CredentialService via remove_account.
        // This IPC remains for governance/history-only callers.
        Ok(())
    }

    fn load_safe_export(&self, generated_at: DateTime<Utc>) -> Result<SafeExport, String> {
        self.database.with_connection(|connection| {
            let quota_snapshots = {
                let mut statement = connection
                    .prepare("SELECT snapshot_json FROM quota_snapshots ORDER BY observed_at_utc")
                    .map_err(|error| error.to_string())?;
                let snapshots = statement
                    .query_map([], |row| row.get::<_, String>(0))
                    .map_err(|error| error.to_string())?;
                let mut rows = Vec::new();
                for snapshot_json in snapshots {
                    let snapshot: QuotaSnapshot = serde_json::from_str(
                        &snapshot_json.map_err(|error| error.to_string())?,
                    )
                    .map_err(|error| format!("stored quota snapshot is invalid: {error}"))?;
                    for window in snapshot.windows {
                        rows.push(QuotaExportRow {
                            observed_at: snapshot.updated_at,
                            account_id: snapshot.account.id.as_str().to_string(),
                            account_display_name: snapshot.account.display_name.clone(),
                            plan_type: snapshot.account.plan_type.clone(),
                            window_name: window.name,
                            remaining_percent: window.remaining_percent,
                            resets_at: window.resets_at,
                            window_duration_minutes: window.window_duration_minutes,
                        });
                    }
                }
                rows
            };
            let token_usage = {
                let mut statement = connection.prepare(
                    "SELECT event.observed_at_utc, attribution.account_key, event.session_id, event.model,
                            event.input_tokens, event.cached_input_tokens, event.cache_write_input_tokens,
                            event.output_tokens, event.reasoning_output_tokens, event.total_tokens
                     FROM token_usage_events event
                     LEFT JOIN token_session_attributions attribution ON attribution.session_id = event.session_id
                     ORDER BY event.observed_at_utc, event.event_key",
                ).map_err(|error| error.to_string())?;
                let rows = statement.query_map([], |row| {
                    Ok(TokenExportRow {
                        observed_at: row.get(0)?,
                        account_id: row.get(1)?,
                        session_id: opaque_session_id(&row.get::<_, String>(2)?),
                        model: row.get(3)?,
                        input_tokens: nonnegative_u64(row.get(4)?),
                        cached_input_tokens: nonnegative_u64(row.get(5)?),
                        cache_write_input_tokens: nonnegative_u64(row.get(6)?),
                        output_tokens: nonnegative_u64(row.get(7)?),
                        reasoning_output_tokens: nonnegative_u64(row.get(8)?),
                        total_tokens: nonnegative_u64(row.get(9)?),
                    })
                }).map_err(|error| error.to_string())?;
                rows.collect::<Result<Vec<_>, _>>().map_err(|error| error.to_string())?
            };
            let system_health = {
                let mut statement = connection.prepare(
                    "SELECT bucket_kind, bucket_start_utc, sample_count, cpu_percent_avg,
                            memory_used_bytes_avg, disk_available_bytes_last, network_down_bps_avg,
                            network_up_bps_avg, battery_percent_last, uptime_seconds_last
                     FROM system_health_aggregates ORDER BY bucket_start_utc, bucket_kind",
                ).map_err(|error| error.to_string())?;
                let rows = statement.query_map([], |row| {
                    Ok(SystemHealthExportRow {
                        bucket_kind: row.get(0)?,
                        observed_at: row.get(1)?,
                        sample_count: nonnegative_u64(row.get(2)?),
                        cpu_percent_avg: row.get(3)?,
                        memory_used_bytes_avg: row.get(4)?,
                        disk_available_bytes_last: nonnegative_u64(row.get(5)?),
                        network_down_bps_avg: row.get(6)?,
                        network_up_bps_avg: row.get(7)?,
                        battery_percent_last: row.get(8)?,
                        uptime_seconds_last: nonnegative_u64(row.get(9)?),
                    })
                }).map_err(|error| error.to_string())?;
                rows.collect::<Result<Vec<_>, _>>().map_err(|error| error.to_string())?
            };
            Ok(SafeExport {
                schema_version: 1,
                generated_at,
                quota_snapshots,
                token_usage,
                system_health,
            })
        })
    }
}

fn delete_time_scoped(
    transaction: &Transaction<'_>,
    table: &str,
    time_column: &str,
    cutoff: Option<&str>,
) -> Result<usize, String> {
    let sql = match cutoff {
        Some(_) => format!("DELETE FROM {table} WHERE {time_column} < ?1"),
        None => format!("DELETE FROM {table}"),
    };
    match cutoff {
        Some(cutoff) => transaction.execute(&sql, params![cutoff]),
        None => transaction.execute(&sql, []),
    }
    .map_err(|error| error.to_string())
}

fn nonnegative_u64(value: i64) -> u64 {
    value.max(0) as u64
}

fn opaque_session_id(session_id: &str) -> String {
    format!(
        "session-{:x}",
        Sha256::digest(format!("export-session:{session_id}").as_bytes())
    )
}

fn csv_cell(value: impl ToString) -> String {
    let mut value = value.to_string();
    if value.starts_with(['=', '+', '-', '@', '\t', '\r']) {
        value.insert(0, '\'');
    }
    if value.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value
    }
}

fn safe_export_csv(export: &SafeExport) -> String {
    const HEADER: &str = "record_type,observed_at,account_id,account_display_name,plan_type,quota_window,remaining_percent,resets_at,window_duration_minutes,session_id,model,input_tokens,cached_input_tokens,cache_write_input_tokens,output_tokens,reasoning_output_tokens,total_tokens,bucket_kind,sample_count,cpu_percent_avg,memory_used_bytes_avg,disk_available_bytes_last,network_down_bps_avg,network_up_bps_avg,battery_percent_last,uptime_seconds_last\n";
    let mut output = String::from(HEADER);
    for row in &export.quota_snapshots {
        let values = vec![
            "quota".to_string(),
            row.observed_at.to_rfc3339(),
            row.account_id.clone(),
            row.account_display_name.clone(),
            row.plan_type.clone(),
            row.window_name.clone(),
            row.remaining_percent.to_string(),
            row.resets_at
                .map(|value| value.to_rfc3339())
                .unwrap_or_default(),
            row.window_duration_minutes
                .map(|value| value.to_string())
                .unwrap_or_default(),
        ];
        output.push_str(&csv_record(values, 26));
    }
    for row in &export.token_usage {
        let mut values = vec![
            "token".to_string(),
            row.observed_at.clone(),
            row.account_id.clone().unwrap_or_default(),
        ];
        values.extend([
            String::new(),
            String::new(),
            String::new(),
            String::new(),
            String::new(),
            String::new(),
        ]);
        values.extend([
            row.session_id.clone(),
            row.model.clone(),
            row.input_tokens.to_string(),
            row.cached_input_tokens.to_string(),
            row.cache_write_input_tokens.to_string(),
            row.output_tokens.to_string(),
            row.reasoning_output_tokens.to_string(),
            row.total_tokens.to_string(),
        ]);
        output.push_str(&csv_record(values, 26));
    }
    for row in &export.system_health {
        let mut values = vec!["system".to_string(), row.observed_at.clone()];
        values.resize(17, String::new());
        values.extend([
            row.bucket_kind.clone(),
            row.sample_count.to_string(),
            row.cpu_percent_avg.to_string(),
            row.memory_used_bytes_avg.to_string(),
            row.disk_available_bytes_last.to_string(),
            row.network_down_bps_avg.to_string(),
            row.network_up_bps_avg.to_string(),
            row.battery_percent_last
                .map(|value| value.to_string())
                .unwrap_or_default(),
            row.uptime_seconds_last.to_string(),
        ]);
        output.push_str(&csv_record(values, 26));
    }
    output
}

fn csv_record(mut values: Vec<String>, columns: usize) -> String {
    values.resize(columns, String::new());
    let mut record = values
        .into_iter()
        .map(csv_cell)
        .collect::<Vec<_>>()
        .join(",");
    record.push('\n');
    record
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf};

    use chrono::{TimeZone, Utc};

    use crate::{
        database::Database,
        lifecycle::{LifecyclePreferences, PreferenceStore},
        quota::{AccountId, QuotaAccount, QuotaSnapshot, QuotaStore, QuotaWindow},
        token_usage::TokenUsageService,
    };

    use super::{CredentialDeletionStatus, DataGovernanceService, ExportFormat, csv_cell};

    #[test]
    fn retention_cleanup_removes_only_history_older_than_the_saved_period() {
        let database = Database::in_memory().unwrap();
        database.with_connection(|connection| {
            connection.execute(
                "INSERT INTO quota_snapshots (account_key, observed_at_utc, snapshot_json) VALUES ('old', '2026-01-01T00:00:00Z', '{}'), ('recent', '2026-07-26T00:00:00Z', '{}')",
                [],
            ).map_err(|error| error.to_string())?;
            connection.execute(
                "INSERT INTO system_health_aggregates (bucket_kind, bucket_start_utc, sample_count, cpu_percent_avg, memory_used_bytes_avg, disk_available_bytes_last, network_down_bps_avg, network_up_bps_avg, battery_percent_last, uptime_seconds_last) VALUES ('minute', '2026-01-01T00:00:00Z', 1, 1, 1, 1, 1, 1, NULL, 1), ('minute', '2026-07-26T00:00:00Z', 1, 1, 1, 1, 1, 1, NULL, 1)",
                [],
            ).map_err(|error| error.to_string())?;
            Ok(())
        }).unwrap();
        let governance = DataGovernanceService::new(database.clone());

        let result = governance
            .cleanup_retention(30, Utc.with_ymd_and_hms(2026, 7, 27, 0, 0, 0).unwrap())
            .unwrap();

        assert_eq!(result.quota_snapshots_deleted, 1);
        assert_eq!(result.system_aggregates_deleted, 1);
        database
            .with_connection(|connection| {
                let quota_count: i64 = connection
                    .query_row("SELECT COUNT(*) FROM quota_snapshots", [], |row| row.get(0))
                    .map_err(|error| error.to_string())?;
                let health_count: i64 = connection
                    .query_row("SELECT COUNT(*) FROM system_health_aggregates", [], |row| {
                        row.get(0)
                    })
                    .map_err(|error| error.to_string())?;
                assert_eq!((quota_count, health_count), (1, 1));
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn clear_history_removes_statistics_without_rewinding_import_checkpoints() {
        let database = Database::in_memory().unwrap();
        database.with_connection(|connection| {
            connection.execute(
                "INSERT INTO token_usage_events (event_key, source_key, observed_at_utc, session_id, model, input_tokens, cached_input_tokens, cache_write_input_tokens, output_tokens, reasoning_output_tokens, total_tokens) VALUES ('event', 'opaque-source', '2026-07-01T00:00:00Z', 'session', 'gpt', 1, 0, 0, 1, 0, 2)",
                [],
            ).map_err(|error| error.to_string())?;
            connection.execute(
                "INSERT INTO token_session_attributions (session_id, account_key, attribution_source, assigned_at_utc) VALUES ('session', NULL, 'unassigned', '2026-07-01T00:00:00Z')",
                [],
            ).map_err(|error| error.to_string())?;
            connection.execute(
                "INSERT INTO token_import_checkpoints (source_key, byte_offset, file_size, parser_context_json, updated_at_utc) VALUES ('opaque-source', 10, 10, '{}', '2026-07-01T00:00:00Z')",
                [],
            ).map_err(|error| error.to_string())?;
            Ok(())
        }).unwrap();
        let governance = DataGovernanceService::new(database.clone());

        let result = governance.clear_history().unwrap();

        assert_eq!(result.token_events_deleted, 1);
        database
            .with_connection(|connection| {
                let event_count: i64 = connection
                    .query_row("SELECT COUNT(*) FROM token_usage_events", [], |row| {
                        row.get(0)
                    })
                    .map_err(|error| error.to_string())?;
                let attribution_count: i64 = connection
                    .query_row(
                        "SELECT COUNT(*) FROM token_session_attributions",
                        [],
                        |row| row.get(0),
                    )
                    .map_err(|error| error.to_string())?;
                let checkpoint_count: i64 = connection
                    .query_row("SELECT COUNT(*) FROM token_import_checkpoints", [], |row| {
                        row.get(0)
                    })
                    .map_err(|error| error.to_string())?;
                assert_eq!(
                    (event_count, attribution_count, checkpoint_count),
                    (0, 0, 1)
                );
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn account_history_deletion_is_scoped_and_does_not_claim_to_delete_credentials() {
        let database = Database::in_memory().unwrap();
        database.with_connection(|connection| {
            for (account, session, event) in [("account-a", "session-a", "event-a"), ("account-b", "session-b", "event-b")] {
                connection.execute(
                    "INSERT INTO token_accounts (account_key, display_name, last_evidence_source, last_evidence_at_utc) VALUES (?1, ?1, 'test', '2026-07-01T00:00:00Z')",
                    [account],
                ).map_err(|error| error.to_string())?;
                connection.execute(
                    "INSERT INTO token_session_attributions (session_id, account_key, attribution_source, assigned_at_utc) VALUES (?1, ?2, 'activeAccount', '2026-07-01T00:00:00Z')",
                    [session, account],
                ).map_err(|error| error.to_string())?;
                connection.execute(
                    "INSERT INTO token_usage_events (event_key, source_key, observed_at_utc, session_id, model, input_tokens, cached_input_tokens, cache_write_input_tokens, output_tokens, reasoning_output_tokens, total_tokens) VALUES (?1, 'source', '2026-07-01T00:00:00Z', ?2, 'gpt', 1, 0, 0, 1, 0, 2)",
                    [event, session],
                ).map_err(|error| error.to_string())?;
            }
            connection.execute(
                "INSERT INTO quota_snapshots (account_key, observed_at_utc, snapshot_json) VALUES ('current-codex-account', '2026-07-01T00:00:00Z', '{\"account\":{\"id\":\"account-a\"}}'), ('account-b', '2026-07-01T00:00:00Z', '{\"account\":{\"id\":\"account-b\"}}')",
                [],
            ).map_err(|error| error.to_string())?;
            Ok(())
        }).unwrap();
        let governance = DataGovernanceService::new(database.clone());

        let result = governance.delete_account_history("account-a").unwrap();

        assert_eq!(result.token_events_deleted, 1);
        assert_eq!(result.quota_snapshots_deleted, 1);
        database
            .with_connection(|connection| {
                let remaining_sessions: String = connection
                    .query_row("SELECT session_id FROM token_usage_events", [], |row| {
                        row.get(0)
                    })
                    .map_err(|error| error.to_string())?;
                let remaining_account: String = connection
                    .query_row("SELECT account_key FROM token_accounts", [], |row| {
                        row.get(0)
                    })
                    .map_err(|error| error.to_string())?;
                assert_eq!(
                    (remaining_sessions.as_str(), remaining_account.as_str()),
                    ("session-b", "account-b")
                );
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn json_and_csv_exports_cover_all_statistics_through_a_privacy_whitelist() {
        let database = Database::in_memory().unwrap();
        database.with_connection(|connection| {
            connection.execute(
                "INSERT INTO quota_snapshots (account_key, observed_at_utc, snapshot_json) VALUES ('account-a', '2026-07-01T00:00:00Z', '{\"account\":{\"id\":\"account-a\",\"displayName\":\"safe@example.com\",\"planType\":\"plus\"},\"windows\":[{\"name\":\"primary\",\"remainingPercent\":80,\"resetsAt\":null,\"windowDurationMinutes\":300}],\"updatedAt\":\"2026-07-01T00:00:00Z\"}')",
                [],
            ).map_err(|error| error.to_string())?;
            connection.execute(
                "INSERT INTO token_usage_events (event_key, source_key, observed_at_utc, session_id, model, input_tokens, cached_input_tokens, cache_write_input_tokens, output_tokens, reasoning_output_tokens, total_tokens) VALUES ('event', '/Users/alice/private/work', '2026-07-01T00:00:01Z', '/Users/alice/private/work', 'gpt-test', 10, 2, 1, 5, 1, 15)",
                [],
            ).map_err(|error| error.to_string())?;
            connection.execute(
                "INSERT INTO system_health_aggregates (bucket_kind, bucket_start_utc, sample_count, cpu_percent_avg, memory_used_bytes_avg, disk_available_bytes_last, network_down_bps_avg, network_up_bps_avg, battery_percent_last, uptime_seconds_last) VALUES ('minute', '2026-07-01T00:00:00Z', 2, 12.5, 1000, 2000, 300, 40, 88, 7200)",
                [],
            ).map_err(|error| error.to_string())?;
            Ok(())
        }).unwrap();
        let governance = DataGovernanceService::new(database);
        let generated_at = Utc.with_ymd_and_hms(2026, 7, 27, 0, 0, 0).unwrap();

        let json = governance.export(ExportFormat::Json, generated_at).unwrap();
        let csv = governance.export(ExportFormat::Csv, generated_at).unwrap();

        for expected in [
            "quotaSnapshots",
            "primary",
            "tokenUsage",
            "gpt-test",
            "systemHealth",
            "12.5",
        ] {
            assert!(
                json.content.contains(expected),
                "JSON should contain {expected}"
            );
        }
        for expected in ["record_type", "quota", "token", "system", "gpt-test"] {
            assert!(
                csv.content.contains(expected),
                "CSV should contain {expected}"
            );
        }
        for prohibited in [
            "access_token",
            "refresh_token",
            "oauth",
            "bearer ",
            "sk-",
            "eyj",
            "prompt",
            "reply",
            "command",
            "attachment",
            "work_path",
            "/users/",
        ] {
            assert!(!json.content.to_ascii_lowercase().contains(prohibited));
            assert!(!csv.content.to_ascii_lowercase().contains(prohibited));
        }
    }

    #[test]
    fn export_reports_only_after_a_real_unique_file_is_written() {
        let governance = DataGovernanceService::new(Database::in_memory().unwrap());
        let directory = tempfile::tempdir().unwrap();
        let generated_at = Utc.with_ymd_and_hms(2026, 7, 27, 0, 0, 0).unwrap();

        let first = governance
            .export_to_directory(
                directory.path(),
                "~/Downloads",
                ExportFormat::Json,
                generated_at,
            )
            .unwrap();
        let second = governance
            .export_to_directory(
                directory.path(),
                "~/Downloads",
                ExportFormat::Json,
                generated_at,
            )
            .unwrap();

        assert_eq!(first.destination, "~/Downloads/codex-usage-2026-07-27.json");
        assert_eq!(
            second.destination,
            "~/Downloads/codex-usage-2026-07-27-1.json"
        );
        assert!(directory.path().join(first.filename).is_file());
        assert!(directory.path().join(second.filename).is_file());
        assert_eq!(
            governance.export_to_directory(
                &directory.path().join("missing"),
                "~/Downloads",
                ExportFormat::Csv,
                generated_at,
            ),
            Err("create export failed".to_string())
        );
    }

    #[test]
    fn credential_deletion_contract_is_available_with_keychain_integration() {
        let governance = DataGovernanceService::new(Database::in_memory().unwrap());

        assert_eq!(
            governance.credential_deletion_status(),
            CredentialDeletionStatus::Available
        );
        assert_eq!(governance.request_credential_deletion("account-a"), Ok(()));
        assert_eq!(
            governance.request_credential_deletion("   "),
            Err("account key is required".to_string())
        );
    }

    #[test]
    fn csv_cells_neutralize_spreadsheet_formulas() {
        assert_eq!(
            csv_cell("=HYPERLINK(\"https://example.invalid\")"),
            "\"'=HYPERLINK(\"\"https://example.invalid\"\")\""
        );
        assert_eq!(csv_cell("+SUM(1,2)"), "\"'+SUM(1,2)\"");
        assert_eq!(csv_cell("safe"), "safe");
    }

    #[test]
    fn sqlite_schema_privacy_scan_has_no_credential_or_session_content_columns() {
        let database = Database::in_memory().unwrap();
        let schema = database.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT sql FROM sqlite_master WHERE type IN ('table', 'index') AND sql IS NOT NULL ORDER BY name",
            ).map_err(|error| error.to_string())?;
            let rows = statement.query_map([], |row| row.get::<_, String>(0)).map_err(|error| error.to_string())?;
            rows.collect::<Result<Vec<_>, _>>().map(|rows| rows.join("\n")).map_err(|error| error.to_string())
        }).unwrap().to_ascii_lowercase();

        for prohibited in [
            "access_token",
            "refresh_token",
            "id_token",
            "prompt",
            "reply",
            "command",
            "attachment",
            "work_path",
            "working_directory",
        ] {
            assert!(
                !schema.contains(prohibited),
                "SQLite schema must not contain {prohibited}"
            );
        }
    }

    #[test]
    fn privacy_regression_scans_sqlite_exports_and_the_absent_log_surface() {
        let directory = tempfile::tempdir().unwrap();
        let database_path = directory.path().join("monitor.sqlite3");
        let database = Database::open(&database_path).unwrap();
        PreferenceStore::save(&database, &LifecyclePreferences::default()).unwrap();
        QuotaStore::save(
            &database,
            &AccountId::from("safe-account"),
            &QuotaSnapshot {
                account: QuotaAccount {
                    id: "safe-account".into(),
                    display_name: "safe@example.com".to_string(),
                    plan_type: "plus".to_string(),
                },
                windows: vec![QuotaWindow {
                    name: "primary".to_string(),
                    remaining_percent: 50,
                    resets_at: None,
                    window_duration_minutes: Some(300),
                }],
                updated_at: Utc.with_ymd_and_hms(2026, 7, 27, 0, 0, 0).unwrap(),
            },
        )
        .unwrap();
        let fixture_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/token");
        let token_usage = TokenUsageService::new(database.clone(), vec![fixture_root]);
        token_usage.scan().unwrap();
        let governance = DataGovernanceService::new(database.clone());
        let export = governance
            .export(
                ExportFormat::Json,
                Utc.with_ymd_and_hms(2026, 7, 27, 0, 0, 0).unwrap(),
            )
            .unwrap();
        drop(governance);
        drop(token_usage);
        drop(database);

        let prohibited = [
            "access_token",
            "refresh_token",
            "id_token",
            "bearer ",
            "sk-",
            "eyj",
            "prompt",
            "reply",
            "command",
            "attachment",
            "work_path",
            "/users/",
        ];
        let mut sqlite_bytes = Vec::new();
        let mut log_outputs = Vec::new();
        for entry in fs::read_dir(directory.path()).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_some_and(|extension| extension == "log") {
                log_outputs.push(path);
            } else if path.is_file() {
                sqlite_bytes.extend(fs::read(path).unwrap());
            }
        }
        let sqlite = String::from_utf8_lossy(&sqlite_bytes).to_ascii_lowercase();
        let export = export.content.to_ascii_lowercase();
        for marker in prohibited {
            assert!(!sqlite.contains(marker), "SQLite leaked {marker}");
            assert!(!export.contains(marker), "export leaked {marker}");
        }
        assert!(
            log_outputs.is_empty(),
            "the app must not create local log output"
        );
    }
}
