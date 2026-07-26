use std::{
    path::Path,
    sync::{Arc, Mutex},
};

use chrono::{DateTime, Timelike, Utc};
use rusqlite::{Connection, params};

use crate::{
    lifecycle::{LifecyclePreferences, PreferenceStore},
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
                 CREATE TABLE IF NOT EXISTS token_import_checkpoints (
                   source_key TEXT PRIMARY KEY,
                   byte_offset INTEGER NOT NULL,
                   file_size INTEGER NOT NULL,
                   parser_context_json TEXT NOT NULL,
                   updated_at_utc TEXT NOT NULL
                 );",
            )
            .map_err(|error| error.to_string())
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
    fn save(&self, snapshot: &QuotaSnapshot) -> Result<(), String> {
        let value = serde_json::to_string(snapshot).map_err(|error| error.to_string())?;
        self.connection
            .lock()
            .map_err(|_| "database lock poisoned".to_string())?
            .execute(
                "INSERT OR REPLACE INTO quota_snapshots (account_key, observed_at_utc, snapshot_json)
                 VALUES (?1, ?2, ?3)",
                params![
                    snapshot.account.display_name,
                    snapshot.updated_at.to_rfc3339(),
                    value
                ],
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

        assert_eq!(database.load().unwrap(), Some(preferences));
    }

    #[test]
    fn latest_quota_snapshot_survives_service_recreation() {
        let database = Database::in_memory().unwrap();
        let snapshot = QuotaSnapshot {
            account: crate::quota::QuotaAccount {
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

        QuotaStore::save(&database, &snapshot).unwrap();

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
}
