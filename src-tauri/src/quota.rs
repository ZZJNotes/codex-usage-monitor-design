use std::sync::{Arc, Mutex, RwLock};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

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
        reason: QuotaErrorReason,
        last_snapshot: Option<QuotaSnapshot>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum QuotaErrorReason {
    Paused,
    Storage,
    Unavailable,
}

pub trait QuotaSource: Send + Sync {
    fn refresh(&self) -> Result<QuotaSnapshot, String>;
}

pub trait QuotaStore: Send + Sync {
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

    pub fn with_store(source: Arc<dyn QuotaSource>, store: Arc<dyn QuotaStore>) -> Self {
        Self {
            source,
            store: Some(store),
            state: RwLock::new(QuotaState::Loading),
            refresh_lock: Mutex::new(()),
        }
    }

    pub fn unavailable(message: String) -> Self {
        let service = Self::new(Arc::new(UnavailableQuotaSource(message.clone())));
        *service.state.write().expect("quota state poisoned") = QuotaState::Error {
            reason: QuotaErrorReason::Unavailable,
            last_snapshot: None,
        };
        service
    }

    pub fn unavailable_with_store(message: String, store: Arc<dyn QuotaStore>) -> Self {
        Self {
            source: Arc::new(UnavailableQuotaSource(message)),
            store: Some(store),
            state: RwLock::new(QuotaState::Error {
                reason: QuotaErrorReason::Unavailable,
                last_snapshot: None,
            }),
            refresh_lock: Mutex::new(()),
        }
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
                    && store.save(&snapshot).is_err()
                {
                    let state = self.error_state(QuotaErrorReason::Storage, Some(snapshot));
                    *self.state.write().expect("quota state poisoned") = state.clone();
                    return state;
                }
                let state = QuotaState::Ready { snapshot };
                *self.state.write().expect("quota state poisoned") = state.clone();
                state
            }
            Err(_) => {
                let state = self.error_state(QuotaErrorReason::Unavailable, None);
                *self.state.write().expect("quota state poisoned") = state.clone();
                state
            }
        }
    }

    pub fn paused(&self) -> QuotaState {
        self.error_state(QuotaErrorReason::Paused, None)
    }

    fn error_state(&self, reason: QuotaErrorReason, fallback: Option<QuotaSnapshot>) -> QuotaState {
        let last_snapshot = match self.latest() {
            QuotaState::Ready { snapshot }
            | QuotaState::Error {
                last_snapshot: Some(snapshot),
                ..
            } => Some(snapshot),
            _ => fallback,
        };
        QuotaState::Error {
            reason,
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

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};
    use serde_json::json;

    use super::*;
    use crate::quota_app_server::{CodexAppServerSource, normalize_responses};

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
    fn persists_a_snapshot_without_restoring_it_to_an_unverified_account() {
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
        );
        assert_eq!(
            service.refresh(),
            QuotaState::Ready {
                snapshot: snapshot.clone()
            }
        );

        let unavailable =
            QuotaService::unavailable_with_store("Codex CLI is unavailable".to_string(), database);

        assert_eq!(
            unavailable.latest(),
            QuotaState::Error {
                reason: QuotaErrorReason::Unavailable,
                last_snapshot: None,
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
