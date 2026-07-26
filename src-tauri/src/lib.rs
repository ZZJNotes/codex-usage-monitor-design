mod commands;
pub mod database;
pub mod governance;
pub mod lifecycle;
pub mod notification;
pub mod platform_metrics;
pub mod quota;
mod quota_app_server;
pub mod system_health;
pub mod token_usage;
mod tray;
mod tray_view;

use std::{
    fs,
    sync::mpsc::{self, RecvTimeoutError},
    sync::{Arc, RwLock},
    thread,
    time::{Duration, Instant},
};

use chrono::{DateTime, Utc};
use commands::NotificationIpcState;
use commands::{
    cleanup_expired_history, clear_history, delete_account_history, export_statistics,
    get_application_status, get_credential_deletion_status, get_lifecycle_preferences,
    get_notification_status, get_quota_state, get_system_health, get_system_health_history,
    get_token_usage, reassign_token_session, recover_quota, refresh_quota, refresh_system_health,
    refresh_token_usage, request_credential_deletion, set_dock_visibility, set_launch_at_login,
    set_locale, set_menu_bar_preferences, set_monitoring_paused, set_notification_preferences,
    set_retention_days, set_theme, show_dashboard, sync_dock_visibility, sync_launch_at_login,
};
use database::Database;
use governance::DataGovernanceService;
use lifecycle::LifecycleService;
use notification::{MacOsNotificationSender, NotificationLocale, NotificationService};
use platform_metrics::MacMetricSource;
use quota::{CURRENT_CODEX_ACCOUNT_ID, QuotaRefreshCoordinator, QuotaService};
use quota_app_server::CodexAppServerSource;
use serde::Serialize;
use system_health::{SystemHealthService, SystemHealthState};
use tauri::{AppHandle, Manager, Runtime, WindowEvent};
use token_usage::{AccountEvidenceSource, ActiveAccountEvidence, TokenAccount, TokenUsageService};
use tray::{TrayMenuItems, setup_tray, update_tray};

pub(crate) struct AppState {
    pub(crate) health: Arc<SystemHealthService>,
    pub(crate) lifecycle: Arc<LifecycleService>,
    pub(crate) quota: Arc<QuotaService>,
    pub(crate) token_usage: Arc<TokenUsageService>,
    pub(crate) governance: Arc<DataGovernanceService>,
    pub(crate) notifications: Arc<NotificationService>,
    pub(crate) application_status: Arc<RwLock<ApplicationStatus>>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ApplicationStatus {
    pub(crate) storage_issue: Option<StorageIssue>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StorageIssue {
    pub(crate) detail: String,
}

impl StorageIssue {
    fn initialization_failed() -> Self {
        Self {
            detail: "storageInitializationFailed".to_string(),
        }
    }

    fn write_failed() -> Self {
        Self {
            detail: "storageWriteFailed".to_string(),
        }
    }

    fn retention_cleanup_failed() -> Self {
        Self {
            detail: "retentionCleanupFailed".to_string(),
        }
    }
}

fn recover_storage_after_successful_health_write(
    application_status: &RwLock<ApplicationStatus>,
    governance: &DataGovernanceService,
    retention_days: u32,
    now: DateTime<Utc>,
) {
    let issue_detail = application_status
        .read()
        .expect("application status poisoned")
        .storage_issue
        .as_ref()
        .map(|issue| issue.detail.clone());
    let recovered_detail = match issue_detail.as_deref() {
        Some("storageWriteFailed") => "storageWriteFailed",
        Some("retentionCleanupFailed") => {
            if governance.cleanup_retention(retention_days, now).is_err() {
                return;
            }
            "retentionCleanupFailed"
        }
        _ => return,
    };
    let mut status = application_status
        .write()
        .expect("application status poisoned");
    if status
        .storage_issue
        .as_ref()
        .is_some_and(|issue| issue.detail == recovered_detail)
    {
        status.storage_issue = None;
    }
}

struct CurrentQuotaAccountEvidence(Arc<QuotaService>);

impl AccountEvidenceSource for CurrentQuotaAccountEvidence {
    fn active_account(&self) -> Option<ActiveAccountEvidence> {
        current_quota_account_evidence(&self.0.latest(), Utc::now())
    }
}

const ACTIVE_ACCOUNT_EVIDENCE_MAX_AGE_SECONDS: i64 = 30;

fn current_quota_account_evidence(
    state: &quota::QuotaState,
    now: DateTime<Utc>,
) -> Option<ActiveAccountEvidence> {
    let quota::QuotaState::Ready { snapshot, .. } = state else {
        return None;
    };
    let age_seconds = (now - snapshot.updated_at).num_seconds();
    if !(0..=ACTIVE_ACCOUNT_EVIDENCE_MAX_AGE_SECONDS).contains(&age_seconds) {
        return None;
    }
    let display_name = snapshot.account.display_name.trim();
    if display_name.is_empty() || display_name == "ChatGPT account" {
        return None;
    }
    Some(ActiveAccountEvidence {
        account: TokenAccount {
            account_key: snapshot.account.id.as_str().to_string(),
            display_name: display_name.to_string(),
        },
        source: "codexAppServerAccountRead".to_string(),
        observed_at: snapshot.updated_at.to_rfc3339(),
    })
}

fn notification_account(
    state: &quota::QuotaState,
    fallback_account_id: &quota::AccountId,
) -> (quota::AccountId, String, bool) {
    let snapshot = state.snapshot();
    snapshot
        .map(|snapshot| {
            (
                snapshot.account.id.clone(),
                snapshot.account.display_name.clone(),
                true,
            )
        })
        .unwrap_or_else(|| {
            (
                fallback_account_id.clone(),
                "Current Codex account".to_string(),
                false,
            )
        })
}

pub(crate) fn set_monitoring_paused_with_account_evidence(
    lifecycle: &LifecycleService,
    quota: &QuotaService,
    paused: bool,
) -> Result<lifecycle::LifecyclePreferences, String> {
    if !paused {
        return lifecycle.resume_after(|| {
            quota.refresh_account_evidence();
        });
    }
    lifecycle.set_monitoring_paused(paused)
}

pub(crate) fn show_main_window<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "dashboard window is unavailable".to_string())?;
    window.show().map_err(|error| error.to_string())?;
    window.unminimize().map_err(|error| error.to_string())?;
    window.set_focus().map_err(|error| error.to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .plugin(tauri_plugin_notification::init())
        .invoke_handler(tauri::generate_handler![
            get_system_health,
            get_system_health_history,
            refresh_system_health,
            get_quota_state,
            refresh_quota,
            get_token_usage,
            refresh_token_usage,
            recover_quota,
            reassign_token_session,
            get_application_status,
            get_lifecycle_preferences,
            get_notification_status,
            set_monitoring_paused,
            set_theme,
            set_locale,
            set_dock_visibility,
            set_launch_at_login,
            set_menu_bar_preferences,
            set_notification_preferences,
            show_dashboard,
            set_retention_days,
            cleanup_expired_history,
            clear_history,
            delete_account_history,
            export_statistics,
            get_credential_deletion_status,
            request_credential_deletion,
        ])
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            let database_result = fs::create_dir_all(&data_dir)
                .map_err(|error| error.to_string())
                .and_then(|_| Database::open(&data_dir.join("monitor.sqlite3")));
            let (database, mut storage_issue, ephemeral_storage) = match database_result {
                Ok(database) => (database, None, false),
                Err(_) => (
                    Database::in_memory().map_err(std::io::Error::other)?,
                    Some(StorageIssue::initialization_failed()),
                    true,
                ),
            };
            let lifecycle = Arc::new(
                LifecycleService::new(Arc::new(database.clone())).map_err(std::io::Error::other)?,
            );
            let governance = Arc::new(DataGovernanceService::new(database.clone()));
            if governance
                .cleanup_retention(lifecycle.preferences().retention_days, Utc::now())
                .is_err()
            {
                storage_issue = Some(StorageIssue::retention_cleanup_failed());
            }
            let health = Arc::new(SystemHealthService::new(Arc::new(MacMetricSource::new())));
            let quota = Arc::new(match CodexAppServerSource::discover() {
                Ok(source) => QuotaService::with_store(
                    CURRENT_CODEX_ACCOUNT_ID,
                    Arc::new(source),
                    Arc::new(database.clone()),
                ),
                Err(message) => QuotaService::unavailable_with_store(
                    CURRENT_CODEX_ACCOUNT_ID,
                    message,
                    Arc::new(database.clone()),
                ),
            });
            let notifications = Arc::new(
                NotificationService::new(
                    Arc::new(database.clone()),
                    Arc::new(MacOsNotificationSender::new(app.handle().clone())),
                )
                .map_err(std::io::Error::other)?,
            );
            if !lifecycle.preferences().monitoring_paused {
                quota.refresh_account_evidence();
            }
            let token_usage = Arc::new(TokenUsageService::with_account_evidence(
                database.clone(),
                token_usage::default_roots(),
                Arc::new(CurrentQuotaAccountEvidence(quota.clone())),
            ));
            let application_status = Arc::new(RwLock::new(ApplicationStatus { storage_issue }));
            app.manage(AppState {
                health: health.clone(),
                lifecycle: lifecycle.clone(),
                quota: quota.clone(),
                token_usage: token_usage.clone(),
                governance: governance.clone(),
                notifications: notifications.clone(),
                application_status: application_status.clone(),
            });
            app.manage(NotificationIpcState {
                lifecycle: lifecycle.clone(),
                notifications: notifications.clone(),
            });
            let preferences = lifecycle.preferences();
            sync_launch_at_login(app.handle(), preferences.launch_at_login)
                .map_err(std::io::Error::other)?;
            app.manage(setup_tray(app.handle(), &preferences)?);
            let tray_app = app.handle().clone();
            thread::spawn(move || {
                loop {
                    thread::sleep(Duration::from_secs(1));
                    let tray = tray_app.state::<TrayMenuItems>();
                    update_tray(&tray_app, &tray);
                }
            });

            sync_dock_visibility(app.handle(), preferences.show_in_dock)
                .map_err(std::io::Error::other)?;

            thread::spawn(move || {
                loop {
                    if let Ok(Some(SystemHealthState::Ready {
                        updated_at,
                        metrics,
                    })) = lifecycle.sample_if_active(&health)
                    {
                        let preferences = lifecycle.preferences();
                        let locale = match preferences.locale {
                            lifecycle::Locale::ZhCn => NotificationLocale::Chinese,
                            lifecycle::Locale::En => NotificationLocale::English,
                        };
                        let _ = notifications.evaluate_system(
                            &SystemHealthState::Ready {
                                updated_at,
                                metrics: metrics.clone(),
                            },
                            &preferences.notifications,
                            locale,
                        );
                        match database.record_health_metrics(updated_at, &metrics) {
                            Ok(()) if !ephemeral_storage => {
                                recover_storage_after_successful_health_write(
                                    &application_status,
                                    &governance,
                                    preferences.retention_days,
                                    Utc::now(),
                                );
                            }
                            Err(_) => {
                                application_status
                                    .write()
                                    .expect("application status poisoned")
                                    .storage_issue = Some(StorageIssue::write_failed());
                                health.report_error("persist system health failed".to_string());
                            }
                            _ => {}
                        }
                    }
                    thread::sleep(Duration::from_secs(2));
                }
            });
            let quota_lifecycle = app.state::<AppState>().lifecycle.clone();
            let quota_notifications = app.state::<AppState>().notifications.clone();
            let notification_quota = quota.clone();
            let quota_refresh = QuotaRefreshCoordinator::new(vec![quota]);
            thread::spawn(move || {
                let mut previous_tick = Utc::now();
                let fallback_notification_account =
                    quota::AccountId::from(CURRENT_CODEX_ACCOUNT_ID);
                loop {
                    if !quota_lifecycle.preferences().monitoring_paused {
                        let now = Utc::now();
                        if (now - previous_tick).num_seconds() > 15 {
                            quota_refresh.stagger_due_recoveries();
                        } else {
                            quota_refresh.refresh_due();
                        }
                        previous_tick = now;
                        let preferences = quota_lifecycle.preferences();
                        let state = notification_quota.latest();
                        let (account_id, display_name, identity_verified) =
                            notification_account(&state, &fallback_notification_account);
                        let locale = match preferences.locale {
                            lifecycle::Locale::ZhCn => NotificationLocale::Chinese,
                            lifecycle::Locale::En => NotificationLocale::English,
                        };
                        if identity_verified {
                            // This read-only source currently observes one account. A future
                            // managed-account coordinator must pass its complete observed set.
                            let _ = quota_notifications.retain_accounts(
                                std::slice::from_ref(&account_id),
                                &preferences.notifications,
                            );
                        }
                        let _ = quota_notifications.evaluate_account(
                            &account_id,
                            &display_name,
                            &state,
                            &preferences.notifications,
                            locale,
                        );
                    }
                    thread::sleep(Duration::from_secs(5));
                }
            });
            let token_lifecycle = app.state::<AppState>().lifecycle.clone();
            thread::spawn(move || {
                if !token_lifecycle.preferences().monitoring_paused {
                    let _ = token_usage.scan();
                }
                let (watch_sender, watch_receiver) = mpsc::channel();
                let _watcher = token_usage.watcher(watch_sender).ok();
                let mut pending_change = false;
                let mut was_paused = token_lifecycle.preferences().monitoring_paused;
                let mut last_reconciliation = Instant::now();
                loop {
                    match watch_receiver.recv_timeout(Duration::from_secs(2)) {
                        Ok(()) => {
                            pending_change = true;
                            while watch_receiver
                                .recv_timeout(Duration::from_millis(300))
                                .is_ok()
                            {}
                        }
                        Err(RecvTimeoutError::Disconnected) => {
                            thread::sleep(Duration::from_secs(2))
                        }
                        Err(RecvTimeoutError::Timeout) => {}
                    }
                    let paused = token_lifecycle.preferences().monitoring_paused;
                    let reconcile = last_reconciliation.elapsed() >= Duration::from_secs(30);
                    if !paused && (pending_change || reconcile || was_paused) {
                        let _ = token_usage.scan();
                        pending_change = false;
                        last_reconciliation = Instant::now();
                    }
                    was_paused = paused;
                }
            });
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running Codex Usage Monitor");
}

#[cfg(test)]
mod account_evidence_tests {
    use std::sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    };

    use chrono::{Duration, Utc};

    use super::*;

    fn quota_snapshot(updated_at: chrono::DateTime<Utc>) -> quota::QuotaSnapshot {
        quota::QuotaSnapshot {
            account: quota::QuotaAccount {
                id: "sanitized-account".into(),
                display_name: "sanitized@example.com".to_string(),
                plan_type: "plus".to_string(),
            },
            windows: Vec::new(),
            updated_at,
        }
    }

    fn quota_state(updated_at: chrono::DateTime<Utc>) -> quota::QuotaState {
        quota::QuotaState::Ready {
            snapshot: quota_snapshot(updated_at),
            next_refresh_at: updated_at + Duration::minutes(10),
        }
    }

    #[test]
    fn only_fresh_quota_observations_are_active_account_evidence() {
        let now = Utc::now();

        assert!(current_quota_account_evidence(&quota_state(now), now).is_some());
        assert!(
            current_quota_account_evidence(&quota_state(now - Duration::seconds(31)), now)
                .is_none()
        );
        assert!(
            current_quota_account_evidence(&quota_state(now + Duration::seconds(1)), now).is_none()
        );
        assert!(
            current_quota_account_evidence(
                &quota::QuotaState::Error {
                    reason: quota::QuotaErrorReason::Unavailable,
                    last_snapshot: None,
                    failed_at: now,
                    retry_at: None,
                },
                now,
            )
            .is_none()
        );
    }

    struct ResumeOrderStore {
        saved: Mutex<lifecycle::LifecyclePreferences>,
    }

    impl lifecycle::PreferenceStore for ResumeOrderStore {
        fn load(&self) -> Result<Option<lifecycle::LifecyclePreferences>, String> {
            Ok(Some(self.saved.lock().unwrap().clone()))
        }

        fn save(&self, preferences: &lifecycle::LifecyclePreferences) -> Result<(), String> {
            *self.saved.lock().unwrap() = preferences.clone();
            Ok(())
        }
    }

    struct ResumeEvidenceSource(Arc<AtomicBool>);

    impl quota::QuotaSource for ResumeEvidenceSource {
        fn refresh(&self) -> Result<quota::QuotaSnapshot, quota::QuotaRefreshError> {
            self.0.store(true, Ordering::SeqCst);
            Ok(quota_snapshot(Utc::now()))
        }
    }

    #[test]
    fn resuming_refreshes_account_evidence_before_monitoring_becomes_active() {
        let refreshed = Arc::new(AtomicBool::new(false));
        let store = Arc::new(ResumeOrderStore {
            saved: Mutex::new(lifecycle::LifecyclePreferences {
                monitoring_paused: true,
                ..lifecycle::LifecyclePreferences::default()
            }),
        });
        let lifecycle = LifecycleService::new(store).unwrap();
        let quota = QuotaService::new(
            "sanitized-account",
            Arc::new(ResumeEvidenceSource(refreshed.clone())),
        );

        let preferences =
            set_monitoring_paused_with_account_evidence(&lifecycle, &quota, false).unwrap();

        assert!(!preferences.monitoring_paused);
        assert!(refreshed.load(Ordering::SeqCst));
    }

    struct FailingResumeStore;

    impl lifecycle::PreferenceStore for FailingResumeStore {
        fn load(&self) -> Result<Option<lifecycle::LifecyclePreferences>, String> {
            Ok(Some(lifecycle::LifecyclePreferences {
                monitoring_paused: true,
                ..lifecycle::LifecyclePreferences::default()
            }))
        }

        fn save(&self, _: &lifecycle::LifecyclePreferences) -> Result<(), String> {
            Err("save failed".to_string())
        }
    }

    #[test]
    fn failed_resume_persistence_keeps_pause_gate_closed_without_network_access() {
        let refreshed = Arc::new(AtomicBool::new(false));
        let lifecycle = LifecycleService::new(Arc::new(FailingResumeStore)).unwrap();
        let quota = QuotaService::new(
            "sanitized-account",
            Arc::new(ResumeEvidenceSource(refreshed.clone())),
        );

        assert_eq!(
            set_monitoring_paused_with_account_evidence(&lifecycle, &quota, false),
            Err("save failed".to_string())
        );
        assert!(lifecycle.preferences().monitoring_paused);
        assert!(!refreshed.load(Ordering::SeqCst));
    }

    #[test]
    fn diagnostics_expose_only_stable_codes_without_local_paths_or_secrets() {
        let diagnostics = serde_json::to_string(&ApplicationStatus {
            storage_issue: Some(StorageIssue::initialization_failed()),
        })
        .unwrap()
        .to_ascii_lowercase();

        assert!(diagnostics.contains("storageinitializationfailed"));
        for prohibited in [
            "/users/",
            "access_token",
            "refresh_token",
            "bearer ",
            "sk-",
            "eyj",
            "prompt",
            "reply",
        ] {
            assert!(!diagnostics.contains(prohibited));
        }
    }

    #[test]
    fn retention_failure_clears_only_after_cleanup_retry_succeeds() {
        let database = Database::in_memory().unwrap();
        let governance = DataGovernanceService::new(database);
        let status = RwLock::new(ApplicationStatus {
            storage_issue: Some(StorageIssue::retention_cleanup_failed()),
        });

        recover_storage_after_successful_health_write(&status, &governance, 30, Utc::now());

        assert!(
            status
                .read()
                .expect("application status poisoned")
                .storage_issue
                .is_none()
        );
    }

    #[test]
    fn successful_health_write_does_not_hide_initialization_failure() {
        let database = Database::in_memory().unwrap();
        let governance = DataGovernanceService::new(database);
        let status = RwLock::new(ApplicationStatus {
            storage_issue: Some(StorageIssue::initialization_failed()),
        });

        recover_storage_after_successful_health_write(&status, &governance, 30, Utc::now());

        assert_eq!(
            status
                .read()
                .expect("application status poisoned")
                .storage_issue
                .as_ref()
                .map(|issue| issue.detail.as_str()),
            Some("storageInitializationFailed")
        );
    }
}
