use std::{
    path::Path,
    sync::{Arc, Mutex},
};

use chrono::{DateTime, Timelike, Utc};
use rusqlite::{Connection, params};

use crate::{
    lifecycle::{LifecyclePreferences, PreferenceStore},
    notification::{NotificationStore, PersistedNotificationState},
    quota::{QuotaSnapshot, QuotaStore},
    system_health::SystemHealthMetrics,
};

#[derive(Clone)]
pub struct Database {
    connection: Arc<Mutex<Connection>>,
}

impl Database {
    pub(crate) fn with_connection<T>(
        &self,
        operation: impl FnOnce(&mut Connection) -> Result<T, String>,
    ) -> Result<T, String> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| "database lock poisoned".to_string())?;
        operation(&mut connection)
    }

    pub fn open(path: &Path) -> Result<Self, String> {
        let connection = Connection::open(path).map_err(|error| error.to_string())?;
        let database = Self {
            connection: Arc::new(Mutex::new(connection)),
        };
        database.migrate()?;
        Ok(database)
    }

    pub fn in_memory() -> Result<Self, String> {
        let connection = Connection::open_in_memory().map_err(|error| error.to_string())?;
        let database = Self {
            connection: Arc::new(Mutex::new(connection)),
        };
        database.migrate()?;
        Ok(database)
    }

    fn migrate(&self) -> Result<(), String> {
        self.connection
            .lock()
            .map_err(|_| "database lock poisoned".to_string())?
            .execute_batch(
                "PRAGMA journal_mode = WAL;
                 PRAGMA foreign_keys = ON;
                 CREATE TABLE IF NOT EXISTS app_preferences (
                   id INTEGER PRIMARY KEY CHECK (id = 1),
                   value_json TEXT NOT NULL,
                   updated_at_utc TEXT NOT NULL
                 );
                 CREATE TABLE IF NOT EXISTS system_health_aggregates (
                   bucket_kind TEXT NOT NULL CHECK (bucket_kind IN ('minute', 'hour')),
                   bucket_start_utc TEXT NOT NULL,
                   sample_count INTEGER NOT NULL,
                   cpu_percent_avg REAL NOT NULL,
                   memory_used_bytes_avg REAL NOT NULL,
                   disk_available_bytes_last INTEGER NOT NULL,
                   network_down_bps_avg REAL NOT NULL,
                   network_up_bps_avg REAL NOT NULL,
                   battery_percent_last REAL,
                   uptime_seconds_last INTEGER NOT NULL,
                   PRIMARY KEY (bucket_kind, bucket_start_utc)
                 );
                 CREATE TABLE IF NOT EXISTS quota_snapshots (
                   account_key TEXT NOT NULL,
                   observed_at_utc TEXT NOT NULL,
                   snapshot_json TEXT NOT NULL,
                   PRIMARY KEY (account_key, observed_at_utc)
                 );
                 CREATE TABLE IF NOT EXISTS notification_state (
                   id INTEGER PRIMARY KEY CHECK (id = 1),
                   value_json TEXT NOT NULL,
                   updated_at_utc TEXT NOT NULL
                 );
                 CREATE TABLE IF NOT EXISTS token_usage_events (
                   event_key TEXT PRIMARY KEY,
                   source_key TEXT NOT NULL,
                   observed_at_utc TEXT NOT NULL,
                   session_id TEXT NOT NULL,
                   model TEXT NOT NULL,
                   input_tokens INTEGER NOT NULL,
                   cached_input_tokens INTEGER NOT NULL,
                   cache_write_input_tokens INTEGER NOT NULL,
                   output_tokens INTEGER NOT NULL,
                   reasoning_output_tokens INTEGER NOT NULL,
                   total_tokens INTEGER NOT NULL
                 );
                 CREATE INDEX IF NOT EXISTS token_usage_time_idx
                   ON token_usage_events(observed_at_utc);
                 CREATE INDEX IF NOT EXISTS token_usage_model_time_idx
                   ON token_usage_events(model, observed_at_utc);
                 CREATE INDEX IF NOT EXISTS token_usage_session_time_idx
                   ON token_usage_events(session_id, observed_at_utc);
                 CREATE TABLE IF NOT EXISTS token_accounts (
                   account_key TEXT PRIMARY KEY,
                   display_name TEXT NOT NULL,
                   last_evidence_source TEXT NOT NULL,
                   last_evidence_at_utc TEXT NOT NULL
                 );
                 CREATE TABLE IF NOT EXISTS token_session_attributions (
                   session_id TEXT PRIMARY KEY,
                   account_key TEXT,
                   attribution_source TEXT NOT NULL CHECK (attribution_source IN ('activeAccount', 'unassigned', 'manual')),
                   assigned_at_utc TEXT NOT NULL,
                   evidence_source TEXT,
                   evidence_observed_at_utc TEXT,
                   FOREIGN KEY (account_key) REFERENCES token_accounts(account_key)
                 );
                 CREATE INDEX IF NOT EXISTS token_attribution_account_idx
                   ON token_session_attributions(account_key);
                 INSERT OR IGNORE INTO token_session_attributions
                   (session_id, account_key, attribution_source, assigned_at_utc,
                    evidence_source, evidence_observed_at_utc)
                   SELECT session_id, NULL, 'unassigned', MIN(observed_at_utc), NULL, NULL
                   FROM token_usage_events GROUP BY session_id;
                 CREATE TABLE IF NOT EXISTS token_import_checkpoints (
                   source_key TEXT PRIMARY KEY,
                   byte_offset INTEGER NOT NULL,
                   file_size INTEGER NOT NULL,
                   parser_context_json TEXT NOT NULL,
                   updated_at_utc TEXT NOT NULL
                 );
                 CREATE TABLE IF NOT EXISTS managed_accounts (
                   account_id TEXT PRIMARY KEY,
                   alias TEXT NOT NULL,
                   identity_fingerprint TEXT NOT NULL,
                   plan_type TEXT NOT NULL,
                   status TEXT NOT NULL,
                   pinned INTEGER NOT NULL DEFAULT 0,
                   delete_intent_json TEXT,
                   created_at_utc TEXT NOT NULL,
                   updated_at_utc TEXT NOT NULL
                 );",
            )
            .map_err(|error| error.to_string())
    }

    pub fn list_managed_accounts(
        &self,
    ) -> Result<Vec<crate::credentials::ManagedAccountRecord>, String> {
        self.with_connection(|connection| {
            let mut statement = connection
                .prepare(
                    "SELECT account_id, alias, identity_fingerprint, plan_type, status, pinned,
                            delete_intent_json, created_at_utc, updated_at_utc
                     FROM managed_accounts ORDER BY created_at_utc",
                )
                .map_err(|error| error.to_string())?;
            let rows = statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, i64>(5)?,
                        row.get::<_, Option<String>>(6)?,
                        row.get::<_, String>(7)?,
                        row.get::<_, String>(8)?,
                    ))
                })
                .map_err(|error| error.to_string())?;
            let mut accounts = Vec::new();
            for row in rows {
                let (
                    account_id,
                    alias,
                    identity_fingerprint,
                    plan_type,
                    status,
                    pinned,
                    delete_intent_json,
                    created_at,
                    updated_at,
                ) = row.map_err(|error| error.to_string())?;
                accounts.push(crate::credentials::ManagedAccountRecord {
                    account_id,
                    alias,
                    identity_fingerprint,
                    plan_type,
                    status: serde_json::from_value(serde_json::Value::String(status.clone()))
                        .or_else(|_| serde_json::from_str(&format!("\"{status}\"")))
                        .map_err(|error| error.to_string())?,
                    pinned: pinned != 0,
                    delete_intent: delete_intent_json
                        .as_deref()
                        .map(serde_json::from_str)
                        .transpose()
                        .map_err(|error| error.to_string())?,
                    created_at,
                    updated_at,
                });
            }
            Ok(accounts)
        })
    }

    pub fn get_managed_account(
        &self,
        account_id: &str,
    ) -> Result<Option<crate::credentials::ManagedAccountRecord>, String> {
        Ok(self
            .list_managed_accounts()?
            .into_iter()
            .find(|account| account.account_id == account_id))
    }

    pub fn upsert_managed_account(
        &self,
        record: &crate::credentials::ManagedAccountRecord,
    ) -> Result<(), String> {
        let status = match serde_json::to_value(record.status) {
            Ok(serde_json::Value::String(value)) => value,
            _ => return Err("invalid managed account status".into()),
        };
        let delete_intent_json = record
            .delete_intent
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|error| error.to_string())?;
        self.with_connection(|connection| {
            connection
                .execute(
                    "INSERT INTO managed_accounts (
                       account_id, alias, identity_fingerprint, plan_type, status, pinned,
                       delete_intent_json, created_at_utc, updated_at_utc
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                     ON CONFLICT(account_id) DO UPDATE SET
                       alias = excluded.alias,
                       identity_fingerprint = excluded.identity_fingerprint,
                       plan_type = excluded.plan_type,
                       status = excluded.status,
                       pinned = excluded.pinned,
                       delete_intent_json = excluded.delete_intent_json,
                       updated_at_utc = excluded.updated_at_utc",
                    params![
                        record.account_id,
                        record.alias,
                        record.identity_fingerprint,
                        record.plan_type,
                        status,
                        if record.pinned { 1 } else { 0 },
                        delete_intent_json,
                        record.created_at,
                        record.updated_at,
                    ],
                )
                .map_err(|error| error.to_string())?;
            Ok(())
        })
    }

    pub fn clear_managed_account_pins(&self) -> Result<(), String> {
        self.with_connection(|connection| {
            connection
                .execute("UPDATE managed_accounts SET pinned = 0", [])
                .map_err(|error| error.to_string())?;
            Ok(())
        })
    }

    pub fn delete_managed_account(&self, account_id: &str) -> Result<(), String> {
        self.with_connection(|connection| {
            connection
                .execute(
                    "DELETE FROM managed_accounts WHERE account_id = ?1",
                    params![account_id],
                )
                .map_err(|error| error.to_string())?;
            Ok(())
        })
    }

    pub fn record_health_metrics(
        &self,
        observed_at: DateTime<Utc>,
        metrics: &SystemHealthMetrics,
    ) -> Result<(), String> {
        let minute = observed_at
            .with_second(0)
            .and_then(|value| value.with_nanosecond(0))
            .unwrap();
        let hour = minute.with_minute(0).unwrap();
        let connection = self
            .connection
            .lock()
            .map_err(|_| "database lock poisoned".to_string())?;
        for (kind, start) in [("minute", minute), ("hour", hour)] {
            connection
                .execute(
                    "INSERT INTO system_health_aggregates (
                       bucket_kind, bucket_start_utc, sample_count, cpu_percent_avg,
                       memory_used_bytes_avg, disk_available_bytes_last,
                       network_down_bps_avg, network_up_bps_avg, battery_percent_last,
                       uptime_seconds_last
                     ) VALUES (?1, ?2, 1, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                     ON CONFLICT(bucket_kind, bucket_start_utc) DO UPDATE SET
                       cpu_percent_avg = (cpu_percent_avg * sample_count + excluded.cpu_percent_avg) / (sample_count + 1),
                       memory_used_bytes_avg = (memory_used_bytes_avg * sample_count + excluded.memory_used_bytes_avg) / (sample_count + 1),
                       network_down_bps_avg = (network_down_bps_avg * sample_count + excluded.network_down_bps_avg) / (sample_count + 1),
                       network_up_bps_avg = (network_up_bps_avg * sample_count + excluded.network_up_bps_avg) / (sample_count + 1),
                       sample_count = sample_count + 1,
                       disk_available_bytes_last = excluded.disk_available_bytes_last,
                       battery_percent_last = excluded.battery_percent_last,
                       uptime_seconds_last = excluded.uptime_seconds_last",
                    params![
                        kind,
                        start.to_rfc3339(),
                        metrics.cpu_percent,
                        metrics.memory_used_bytes as f64,
                        metrics.disk_available_bytes.min(i64::MAX as u64) as i64,
                        metrics.network_down_bytes_per_second,
                        metrics.network_up_bytes_per_second,
                        metrics.battery_percent,
                        metrics.uptime_seconds.min(i64::MAX as u64) as i64,
                    ],
                )
                .map_err(|error| error.to_string())?;
        }
        Ok(())
    }
}

impl PreferenceStore for Database {
    fn load(&self) -> Result<Option<LifecyclePreferences>, String> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| "database lock poisoned".to_string())?;
        let result = connection.query_row(
            "SELECT value_json FROM app_preferences WHERE id = 1",
            [],
            |row| row.get::<_, String>(0),
        );
        match result {
            Ok(value) => serde_json::from_str(&value)
                .map(Some)
                .map_err(|error| error.to_string()),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(error) => Err(error.to_string()),
        }
    }

    fn save(&self, preferences: &LifecyclePreferences) -> Result<(), String> {
        let value = serde_json::to_string(preferences).map_err(|error| error.to_string())?;
        self.connection
            .lock()
            .map_err(|_| "database lock poisoned".to_string())?
            .execute(
                "INSERT INTO app_preferences (id, value_json, updated_at_utc)
                 VALUES (1, ?1, ?2)
                 ON CONFLICT(id) DO UPDATE SET value_json = excluded.value_json,
                   updated_at_utc = excluded.updated_at_utc",
                params![value, Utc::now().to_rfc3339()],
            )
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
}

impl QuotaStore for Database {
    fn load(&self, account_id: &crate::quota::AccountId) -> Result<Option<QuotaSnapshot>, String> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| "database lock poisoned".to_string())?;
        let result = connection.query_row(
            "SELECT snapshot_json FROM quota_snapshots WHERE account_key = ?1 ORDER BY observed_at_utc DESC LIMIT 1",
            params![account_id.as_str()],
            |row| row.get::<_, String>(0),
        );
        match result {
            Ok(value) => serde_json::from_str(&value)
                .map(Some)
                .map_err(|error| error.to_string()),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(error) => Err(error.to_string()),
        }
    }

    fn save(
        &self,
        account_id: &crate::quota::AccountId,
        snapshot: &QuotaSnapshot,
    ) -> Result<(), String> {
        let value = serde_json::to_string(snapshot).map_err(|error| error.to_string())?;
        self.connection
            .lock()
            .map_err(|_| "database lock poisoned".to_string())?
            .execute(
                "INSERT OR REPLACE INTO quota_snapshots (account_key, observed_at_utc, snapshot_json)
                 VALUES (?1, ?2, ?3)",
                params![
                    account_id.as_str(),
                    snapshot.updated_at.to_rfc3339(),
                    value
                ],
            )
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
}

impl NotificationStore for Database {
    fn load_notification_state(&self) -> Result<Option<PersistedNotificationState>, String> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| "database lock poisoned".to_string())?;
        let result = connection.query_row(
            "SELECT value_json FROM notification_state WHERE id = 1",
            [],
            |row| row.get::<_, String>(0),
        );
        match result {
            Ok(value) => serde_json::from_str(&value)
                .map(Some)
                .map_err(|error| error.to_string()),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(error) => Err(error.to_string()),
        }
    }

    fn save_notification_state(&self, state: &PersistedNotificationState) -> Result<(), String> {
        let value = serde_json::to_string(state).map_err(|error| error.to_string())?;
        self.connection
            .lock()
            .map_err(|_| "database lock poisoned".to_string())?
            .execute(
                "INSERT INTO notification_state (id, value_json, updated_at_utc)
                 VALUES (1, ?1, ?2)
                 ON CONFLICT(id) DO UPDATE SET value_json = excluded.value_json,
                   updated_at_utc = excluded.updated_at_utc",
                params![value, Utc::now().to_rfc3339()],
            )
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifecycle_preferences_survive_service_recreation() {
        let database = Database::in_memory().unwrap();
        let preferences = LifecyclePreferences {
            monitoring_paused: true,
            theme: crate::lifecycle::Theme::Dark,
            ..LifecyclePreferences::default()
        };

        PreferenceStore::save(&database, &preferences).unwrap();

        assert_eq!(PreferenceStore::load(&database).unwrap(), Some(preferences));
    }

    #[test]
    fn preferences_saved_before_menu_bar_configuration_receive_safe_defaults() {
        let database = Database::in_memory().unwrap();
        database
            .connection
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO app_preferences (id, value_json, updated_at_utc) VALUES (1, ?1, ?2)",
                params![
                    r#"{"monitoringPaused":false,"locale":"zh-CN","theme":"system","showInDock":false,"launchAtLogin":false}"#,
                    Utc::now().to_rfc3339()
                ],
            )
            .unwrap();

        let loaded = PreferenceStore::load(&database).unwrap().unwrap();

        assert_eq!(
            loaded.menu_bar,
            crate::lifecycle::MenuBarPreferences::default()
        );
    }

    #[test]
    fn preferences_created_before_retention_support_load_with_the_safe_default() {
        let database = Database::in_memory().unwrap();
        database.with_connection(|connection| {
            connection.execute(
                "INSERT INTO app_preferences (id, value_json, updated_at_utc) VALUES (1, ?1, ?2)",
                params![r#"{"monitoringPaused":true,"locale":"zh-CN","theme":"system","showInDock":false,"launchAtLogin":false}"#, Utc::now().to_rfc3339()],
            ).map(|_| ()).map_err(|error| error.to_string())
        }).unwrap();

        let preferences = PreferenceStore::load(&database).unwrap().unwrap();

        assert!(preferences.monitoring_paused);
        assert_eq!(preferences.retention_days, 90);
    }

    #[test]
    fn latest_quota_snapshot_survives_service_recreation() {
        let database = Database::in_memory().unwrap();
        let snapshot = QuotaSnapshot {
            account: crate::quota::QuotaAccount {
                id: "account-1".into(),
                display_name: "user@example.com".to_string(),
                plan_type: "plus".to_string(),
            },
            windows: vec![crate::quota::QuotaWindow {
                name: "codex · primary".to_string(),
                remaining_percent: 80,
                resets_at: None,
                window_duration_minutes: Some(300),
            }],
            updated_at: Utc::now(),
        };

        let account_id = crate::quota::AccountId::from("account-1");
        QuotaStore::save(&database, &account_id, &snapshot).unwrap();

        let stored = database
            .connection
            .lock()
            .unwrap()
            .query_row(
                "SELECT snapshot_json FROM quota_snapshots ORDER BY observed_at_utc DESC LIMIT 1",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap();
        assert_eq!(
            serde_json::from_str::<QuotaSnapshot>(&stored).unwrap(),
            snapshot
        );
    }

    #[test]
    fn notification_deduplication_state_survives_service_recreation() {
        let database = Database::in_memory().unwrap();
        let state = crate::notification::PersistedNotificationState::default();

        crate::notification::NotificationStore::save_notification_state(&database, &state).unwrap();

        assert_eq!(
            crate::notification::NotificationStore::load_notification_state(&database).unwrap(),
            Some(state)
        );
    }
}
