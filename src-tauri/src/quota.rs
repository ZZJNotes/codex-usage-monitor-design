use std::sync::{Arc, Mutex, RwLock};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaAccount {
    pub display_name: String,
    pub plan_type: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaWindow {
    pub name: String,
    pub remaining_percent: u8,
    pub resets_at: Option<DateTime<Utc>>,
    pub window_duration_minutes: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaSnapshot {
    pub account: QuotaAccount,
    pub windows: Vec<QuotaWindow>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum QuotaState {
    Loading,
    Ready {
        snapshot: QuotaSnapshot,
    },
    Error {
        message: String,
        last_snapshot: Option<QuotaSnapshot>,
    },
}

pub trait QuotaSource: Send + Sync {
    fn refresh(&self) -> Result<QuotaSnapshot, String>;
}

pub trait QuotaStore: Send + Sync {
    fn load_latest(&self) -> Result<Option<QuotaSnapshot>, String>;
    fn save(&self, snapshot: &QuotaSnapshot) -> Result<(), String>;
}

pub struct QuotaService {
    source: Arc<dyn QuotaSource>,
    store: Option<Arc<dyn QuotaStore>>,
    state: RwLock<QuotaState>,
    refresh_lock: Mutex<()>,
}

impl QuotaService {
    pub fn new(source: Arc<dyn QuotaSource>) -> Self {
        Self {
            source,
            store: None,
            state: RwLock::new(QuotaState::Loading),
            refresh_lock: Mutex::new(()),
        }
    }

    pub fn with_store(
        source: Arc<dyn QuotaSource>,
        store: Arc<dyn QuotaStore>,
    ) -> Result<Self, String> {
        let initial = store
            .load_latest()?
            .map_or(QuotaState::Loading, |snapshot| QuotaState::Ready {
                snapshot,
            });
        Ok(Self {
            source,
            store: Some(store),
            state: RwLock::new(initial),
            refresh_lock: Mutex::new(()),
        })
    }

    pub fn unavailable(message: String) -> Self {
        let service = Self::new(Arc::new(UnavailableQuotaSource(message.clone())));
        *service.state.write().expect("quota state poisoned") = QuotaState::Error {
            message,
            last_snapshot: None,
        };
        service
    }

    pub fn unavailable_with_store(
        message: String,
        store: Arc<dyn QuotaStore>,
    ) -> Result<Self, String> {
        let last_snapshot = store.load_latest()?;
        Ok(Self {
            source: Arc::new(UnavailableQuotaSource(message.clone())),
            store: Some(store),
            state: RwLock::new(QuotaState::Error {
                message,
                last_snapshot,
            }),
            refresh_lock: Mutex::new(()),
        })
    }

    pub fn latest(&self) -> QuotaState {
        self.state.read().expect("quota state poisoned").clone()
    }

    pub fn refresh(&self) -> QuotaState {
        let _refresh = self
            .refresh_lock
            .lock()
            .expect("quota refresh lock poisoned");
        match self.source.refresh() {
            Ok(snapshot) => {
                if let Some(store) = &self.store
                    && let Err(message) = store.save(&snapshot)
                {
                    let state = QuotaState::Error {
                        message,
                        last_snapshot: Some(snapshot),
                    };
                    *self.state.write().expect("quota state poisoned") = state.clone();
                    return state;
                }
                let state = QuotaState::Ready { snapshot };
                *self.state.write().expect("quota state poisoned") = state.clone();
                state
            }
            Err(message) => {
                let last_snapshot = match self.latest() {
                    QuotaState::Ready { snapshot }
                    | QuotaState::Error {
                        last_snapshot: Some(snapshot),
                        ..
                    } => Some(snapshot),
                    _ => None,
                };
                let state = QuotaState::Error {
                    message,
                    last_snapshot,
                };
                *self.state.write().expect("quota state poisoned") = state.clone();
                state
            }
        }
    }

    pub fn paused(&self) -> QuotaState {
        let last_snapshot = match self.latest() {
            QuotaState::Ready { snapshot }
            | QuotaState::Error {
                last_snapshot: Some(snapshot),
                ..
            } => Some(snapshot),
            _ => None,
        };
        QuotaState::Error {
            message: "monitoring_paused".to_string(),
            last_snapshot,
        }
    }
}

struct UnavailableQuotaSource(String);

impl QuotaSource for UnavailableQuotaSource {
    fn refresh(&self) -> Result<QuotaSnapshot, String> {
        Err(self.0.clone())
    }
}

pub(crate) fn normalize_responses(
    account_result: &Value,
    rate_limit_result: &Value,
    observed_at: DateTime<Utc>,
) -> Result<QuotaSnapshot, String> {
    let account = account_result
        .get("account")
        .filter(|account| account.get("type").and_then(Value::as_str) == Some("chatgpt"))
        .ok_or_else(|| "The current Codex login is not a ChatGPT account".to_string())?;
    let display_name = account
        .get("email")
        .and_then(Value::as_str)
        .unwrap_or("ChatGPT account")
        .to_string();
    let plan_type = account
        .get("planType")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_string();
    let mut buckets = Vec::new();
    if let Some(by_limit_id) = rate_limit_result
        .get("rateLimitsByLimitId")
        .and_then(Value::as_object)
        .filter(|value| !value.is_empty())
    {
        let mut entries = by_limit_id.iter().collect::<Vec<_>>();
        entries.sort_by_key(|(key, _)| *key);
        for (key, snapshot) in entries {
            buckets.push((key.as_str(), snapshot));
        }
    } else if let Some(snapshot) = rate_limit_result.get("rateLimits") {
        let key = snapshot
            .get("limitId")
            .and_then(Value::as_str)
            .unwrap_or("codex");
        buckets.push((key, snapshot));
    }
    let mut windows = Vec::new();
    for (bucket_key, snapshot) in buckets {
        let bucket_name = snapshot
            .get("limitName")
            .and_then(Value::as_str)
            .or_else(|| snapshot.get("limitId").and_then(Value::as_str))
            .unwrap_or(bucket_key);
        let mut named_windows = snapshot
            .as_object()
            .into_iter()
            .flat_map(|fields| fields.iter())
            .filter(|(_, value)| value.get("usedPercent").is_some())
            .collect::<Vec<_>>();
        named_windows.sort_by_key(|(name, _)| match name.as_str() {
            "primary" => (0, name.as_str()),
            "secondary" => (1, name.as_str()),
            _ => (2, name.as_str()),
        });
        for (window_name, window) in named_windows {
            let used_percent = window
                .get("usedPercent")
                .and_then(Value::as_i64)
                .filter(|value| (0..=100).contains(value))
                .ok_or_else(|| {
                    format!("{bucket_name} {window_name} has an invalid quota percentage")
                })?;
            let resets_at = window
                .get("resetsAt")
                .and_then(Value::as_i64)
                .map(|timestamp| {
                    DateTime::from_timestamp(timestamp, 0).ok_or_else(|| {
                        format!("{bucket_name} {window_name} has an invalid reset time")
                    })
                })
                .transpose()?;
            windows.push(QuotaWindow {
                name: format!("{bucket_name} · {window_name}"),
                remaining_percent: (100 - used_percent) as u8,
                resets_at,
                window_duration_minutes: window.get("windowDurationMins").and_then(Value::as_u64),
            });
        }
    }
    Ok(QuotaSnapshot {
        account: QuotaAccount {
            display_name,
            plan_type,
        },
        windows,
        updated_at: observed_at,
    })
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};
    use serde_json::json;

    use super::*;
    use crate::quota_app_server::CodexAppServerSource;

    struct StaticQuotaSource(QuotaSnapshot);

    impl QuotaSource for StaticQuotaSource {
        fn refresh(&self) -> Result<QuotaSnapshot, String> {
            Ok(self.0.clone())
        }
    }

    #[test]
    fn normalizes_named_windows_to_remaining_percentage() {
        let account = json!({
            "account": { "type": "chatgpt", "email": "user@example.com", "planType": "plus" },
            "requiresOpenaiAuth": true
        });
        let rate_limits = json!({
            "rateLimits": {},
            "rateLimitsByLimitId": {
                "codex": {
                    "limitId": "codex",
                    "limitName": null,
                    "planType": "plus",
                    "primary": { "usedPercent": 15, "windowDurationMins": 10080, "resetsAt": 1785660345 },
                    "secondary": null
                }
            }
        });
        let observed_at = Utc.with_ymd_and_hms(2026, 7, 26, 12, 0, 0).unwrap();

        let snapshot = normalize_responses(&account, &rate_limits, observed_at).unwrap();

        assert_eq!(snapshot.account.display_name, "user@example.com");
        assert_eq!(snapshot.account.plan_type, "plus");
        assert_eq!(snapshot.windows.len(), 1);
        assert_eq!(snapshot.windows[0].name, "codex · primary");
        assert_eq!(snapshot.windows[0].remaining_percent, 85);
        assert_eq!(snapshot.windows[0].window_duration_minutes, Some(10080));
        assert_eq!(snapshot.updated_at, observed_at);
    }

    #[test]
    fn rejects_invalid_percentages_instead_of_inventing_quota() {
        let account = json!({
            "account": { "type": "chatgpt", "email": null, "planType": "plus" }
        });
        let rate_limits = json!({
            "rateLimits": {
                "limitId": "codex",
                "primary": { "usedPercent": 140 }
            },
            "rateLimitsByLimitId": null
        });

        assert!(normalize_responses(&account, &rate_limits, Utc::now()).is_err());
    }

    #[test]
    fn preserves_additional_named_windows_from_future_protocol_versions() {
        let account = json!({
            "account": { "type": "chatgpt", "email": "user@example.com", "planType": "team" }
        });
        let rate_limits = json!({
            "rateLimits": {
                "limitId": "codex",
                "primary": { "usedPercent": 10 },
                "monthly": { "usedPercent": 25, "windowDurationMins": 43200 },
                "metadata": { "future": true }
            }
        });

        let snapshot = normalize_responses(&account, &rate_limits, Utc::now()).unwrap();

        assert_eq!(
            snapshot
                .windows
                .iter()
                .map(|window| window.name.as_str())
                .collect::<Vec<_>>(),
            vec!["codex · primary", "codex · monthly"]
        );
        assert_eq!(snapshot.windows[1].remaining_percent, 75);
    }

    #[test]
    fn restores_the_last_snapshot_when_the_cli_is_unavailable_after_restart() {
        let database = Arc::new(crate::database::Database::in_memory().unwrap());
        let snapshot = QuotaSnapshot {
            account: QuotaAccount {
                display_name: "user@example.com".to_string(),
                plan_type: "plus".to_string(),
            },
            windows: vec![],
            updated_at: Utc::now(),
        };
        let service = QuotaService::with_store(
            Arc::new(StaticQuotaSource(snapshot.clone())),
            database.clone(),
        )
        .unwrap();
        assert_eq!(
            service.refresh(),
            QuotaState::Ready {
                snapshot: snapshot.clone()
            }
        );

        let restored =
            QuotaService::unavailable_with_store("Codex CLI is unavailable".to_string(), database)
                .unwrap();

        assert_eq!(
            restored.latest(),
            QuotaState::Error {
                message: "Codex CLI is unavailable".to_string(),
                last_snapshot: Some(snapshot),
            }
        );
    }

    #[test]
    #[ignore = "requires a locally authenticated Codex CLI"]
    fn reads_live_quota_without_exposing_credentials() {
        let snapshot = CodexAppServerSource::discover().unwrap().refresh().unwrap();

        assert!(!snapshot.account.display_name.is_empty());
        assert!(
            snapshot
                .windows
                .iter()
                .all(|window| window.remaining_percent <= 100)
        );
    }
}
