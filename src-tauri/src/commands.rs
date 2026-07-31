use std::sync::Arc;

use chrono::Utc;
use tauri::{AppHandle, Manager, State};
use tauri_plugin_autostart::ManagerExt;

use crate::{
    AppState, ApplicationStatus,
    credentials::DiscoveredAccount,
    governance::{CredentialDeletionStatus, ExportFormat, ExportReceipt, HistoryCleanupResult},
    lifecycle::{LifecyclePreferences, MenuBarPreferences},
    notification::{NotificationPolicy, NotificationService, NotificationStatus},
    quota::{AccountSummary, QuotaService, QuotaState},
    set_monitoring_paused_with_account_evidence, show_main_window,
    system_health::{StaleReason, SystemHealthPoint, SystemHealthState},
    token_usage::{TokenUsageFilters, TokenUsageService, TokenUsageState},
    tray::update_tray_title,
};

pub(crate) struct NotificationIpcState {
    pub(crate) lifecycle: Arc<crate::lifecycle::LifecycleService>,
    pub(crate) notifications: Arc<NotificationService>,
}

#[tauri::command]
pub(crate) fn get_system_health(state: State<'_, AppState>) -> SystemHealthState {
    visible_system_health(&state)
}

fn visible_system_health(state: &AppState) -> SystemHealthState {
    let paused = state.lifecycle.preferences().monitoring_paused;
    match state.health.latest() {
        SystemHealthState::Ready {
            updated_at,
            metrics,
        } if paused || (Utc::now() - updated_at).num_seconds() > 10 => SystemHealthState::Stale {
            updated_at,
            metrics,
            reason: if paused {
                StaleReason::Paused
            } else {
                StaleReason::Outdated
            },
        },
        current => current,
    }
}

#[tauri::command]
pub(crate) fn refresh_system_health(state: State<'_, AppState>) -> SystemHealthState {
    let _ = state.lifecycle.sample_if_active(&state.health);
    visible_system_health(&state)
}

#[tauri::command]
pub(crate) fn get_system_health_history(state: State<'_, AppState>) -> Vec<SystemHealthPoint> {
    state.health.history()
}

#[tauri::command]
pub(crate) fn get_quota_state(state: State<'_, AppState>) -> QuotaState {
    visible_quota_state(
        state.lifecycle.preferences().monitoring_paused,
        &state.quota,
    )
}

fn visible_quota_state(paused: bool, quota: &QuotaService) -> QuotaState {
    if paused {
        quota.paused()
    } else {
        quota.latest()
    }
}

#[tauri::command]
pub(crate) fn refresh_quota(state: State<'_, AppState>, app: AppHandle) -> QuotaState {
    let result = refresh_quota_service(
        state.lifecycle.preferences().monitoring_paused,
        &state.quota,
    );
    update_tray_title(&app);
    result
}

#[tauri::command]
pub(crate) fn recover_quota(state: State<'_, AppState>, app: AppHandle) -> QuotaState {
    let result = recover_quota_service(
        state.lifecycle.preferences().monitoring_paused,
        &state.quota,
    );
    update_tray_title(&app);
    result
}

fn refresh_quota_service(paused: bool, quota: &QuotaService) -> QuotaState {
    if paused {
        quota.paused()
    } else {
        quota.manual_refresh()
    }
}

fn recover_quota_service(paused: bool, quota: &QuotaService) -> QuotaState {
    if paused {
        quota.paused()
    } else {
        quota.recover_if_due()
    }
}

#[tauri::command]
pub(crate) fn get_token_usage(
    filters: TokenUsageFilters,
    state: State<'_, AppState>,
) -> TokenUsageState {
    let current = state.token_usage.query(filters);
    if state.lifecycle.preferences().monitoring_paused {
        current.paused()
    } else {
        current
    }
}

#[tauri::command]
pub(crate) fn refresh_token_usage(
    filters: TokenUsageFilters,
    state: State<'_, AppState>,
) -> TokenUsageState {
    refresh_token_usage_service(
        state.lifecycle.preferences().monitoring_paused,
        &state.token_usage,
        filters,
    )
}

fn refresh_token_usage_service(
    paused: bool,
    token_usage: &TokenUsageService,
    filters: TokenUsageFilters,
) -> TokenUsageState {
    if paused {
        return token_usage.query(filters).paused();
    }
    let _ = token_usage.scan();
    token_usage.query(filters)
}

#[tauri::command]
pub(crate) fn reassign_token_session(
    session_id: String,
    account_key: Option<String>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state.token_usage.reassign_session(
        &session_id,
        account_key.as_deref(),
        &Utc::now().to_rfc3339(),
    )
}

#[tauri::command]
pub(crate) fn get_application_status(state: State<'_, AppState>) -> ApplicationStatus {
    state
        .application_status
        .read()
        .expect("application status poisoned")
        .clone()
}

#[tauri::command]
pub(crate) fn get_lifecycle_preferences(state: State<'_, AppState>) -> LifecyclePreferences {
    state.lifecycle.preferences()
}

#[tauri::command]
pub(crate) fn get_notification_status(
    state: State<'_, NotificationIpcState>,
) -> NotificationStatus {
    state.notifications.status()
}

#[tauri::command]
pub(crate) fn set_monitoring_paused(
    paused: bool,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<LifecyclePreferences, String> {
    let preferences =
        set_monitoring_paused_with_account_evidence(&state.lifecycle, &state.quota, paused)?;
    update_tray_title(&app);
    Ok(preferences)
}

#[tauri::command]
pub(crate) fn set_theme(
    theme: String,
    state: State<'_, AppState>,
) -> Result<LifecyclePreferences, String> {
    state.lifecycle.set_theme(&theme)
}

#[tauri::command]
pub(crate) fn set_dock_visibility(
    show_in_dock: bool,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<LifecyclePreferences, String> {
    let previous = state.lifecycle.preferences().show_in_dock;
    sync_and_persist_boolean_preference(
        previous,
        show_in_dock,
        |value| sync_dock_visibility(&app, value),
        |value| state.lifecycle.set_show_in_dock(value),
    )
}

pub(crate) fn sync_dock_visibility(app: &AppHandle, show_in_dock: bool) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        app.set_activation_policy(if show_in_dock {
            tauri::ActivationPolicy::Regular
        } else {
            tauri::ActivationPolicy::Accessory
        })
        .map_err(|_| "Dock visibility update failed".to_string())
    }
    #[cfg(not(target_os = "macos"))]
    Ok(())
}

pub(crate) fn sync_launch_at_login(app: &AppHandle, enabled: bool) -> Result<(), String> {
    let manager = app.autolaunch();
    if enabled {
        manager.enable()
    } else {
        manager.disable()
    }
    .map_err(|_| "launch-at-login update failed".to_string())
}

#[tauri::command]
pub(crate) fn set_launch_at_login(
    launch_at_login: bool,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<LifecyclePreferences, String> {
    let previous = state.lifecycle.preferences().launch_at_login;
    sync_and_persist_boolean_preference(
        previous,
        launch_at_login,
        |value| sync_launch_at_login(&app, value),
        |value| state.lifecycle.set_launch_at_login(value),
    )
}

fn sync_and_persist_boolean_preference(
    previous: bool,
    requested: bool,
    sync_system: impl Fn(bool) -> Result<(), String>,
    persist: impl FnOnce(bool) -> Result<LifecyclePreferences, String>,
) -> Result<LifecyclePreferences, String> {
    sync_system(requested)?;
    match persist(requested) {
        Ok(preferences) => Ok(preferences),
        Err(error) => {
            let _ = sync_system(previous);
            Err(error)
        }
    }
}

#[tauri::command]
pub(crate) fn set_retention_days(
    retention_days: u32,
    state: State<'_, AppState>,
) -> Result<LifecyclePreferences, String> {
    let preferences = state.lifecycle.set_retention_days(retention_days)?;
    state
        .governance
        .cleanup_retention(preferences.retention_days, Utc::now())?;
    Ok(preferences)
}

#[tauri::command]
pub(crate) fn cleanup_expired_history(
    state: State<'_, AppState>,
) -> Result<HistoryCleanupResult, String> {
    state
        .governance
        .cleanup_retention(state.lifecycle.preferences().retention_days, Utc::now())
}

#[tauri::command]
pub(crate) fn clear_history(state: State<'_, AppState>) -> Result<HistoryCleanupResult, String> {
    let result = state.governance.clear_history()?;
    state.health.clear_history();
    Ok(result)
}

#[tauri::command]
pub(crate) fn delete_account_history(
    account_key: String,
    state: State<'_, AppState>,
) -> Result<HistoryCleanupResult, String> {
    state.governance.delete_account_history(&account_key)
}

#[tauri::command]
pub(crate) fn export_statistics(
    format: ExportFormat,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<ExportReceipt, String> {
    let downloads = app
        .path()
        .download_dir()
        .map_err(|_| "downloads directory is unavailable".to_string())?;
    state
        .governance
        .export_to_directory(&downloads, "~/Downloads", format, Utc::now())
}

#[tauri::command]
pub(crate) fn get_credential_deletion_status(
    state: State<'_, AppState>,
) -> CredentialDeletionStatus {
    state.governance.credential_deletion_status()
}

#[tauri::command]
pub(crate) fn request_credential_deletion(
    account_key: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state.governance.request_credential_deletion(&account_key)
}

#[tauri::command]
pub(crate) fn set_locale(
    locale: String,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<LifecyclePreferences, String> {
    let preferences = state.lifecycle.set_locale(&locale)?;
    update_tray_title(&app);
    Ok(preferences)
}

#[tauri::command]
pub(crate) fn set_menu_bar_preferences(
    menu_bar: MenuBarPreferences,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<LifecyclePreferences, String> {
    let preferences = state.lifecycle.set_menu_bar(menu_bar)?;
    update_tray_title(&app);
    Ok(preferences)
}

#[tauri::command]
pub(crate) fn set_notification_preferences(
    notifications: NotificationPolicy,
    state: State<'_, NotificationIpcState>,
) -> Result<LifecyclePreferences, String> {
    state.lifecycle.set_notifications(notifications)
}

#[tauri::command]
pub(crate) fn show_dashboard(app: AppHandle) -> Result<(), String> {
    show_main_window(&app)
}

#[tauri::command]
pub(crate) fn quit_app(app: AppHandle) {
    app.exit(0);
}

/// List managed accounts (secret-free DTO). Does not scan CLIProxyAPI/Codex auth files.
#[tauri::command]
pub(crate) fn discover_accounts(state: State<'_, AppState>) -> Vec<DiscoveredAccount> {
    let mut accounts = state.credentials.discover_accounts();
    // Always expose the current Codex fallback as a non-managed entry when available.
    if let Some(snapshot) = state.quota.latest().snapshot() {
        accounts.insert(
            0,
            DiscoveredAccount {
                account_key: crate::quota::CURRENT_CODEX_ACCOUNT_ID.to_string(),
                display_name: snapshot.account.display_name.clone(),
                auth_source: "active".to_string(),
                is_managed: false,
                status: None,
                pinned: false,
            },
        );
    }
    accounts
}

/// List all tracked account summaries from the refresh coordinator.
#[tauri::command]
pub(crate) fn list_accounts(state: State<'_, AppState>) -> Vec<AccountSummary> {
    let coordinator = state
        .quota_refresh
        .lock()
        .expect("quota refresh lock poisoned");
    coordinator.account_summaries()
}

/// Return independent quota states for every tracked account.
#[tauri::command]
pub(crate) fn get_all_quotas(state: State<'_, AppState>) -> Vec<(String, QuotaState)> {
    let coordinator = state
        .quota_refresh
        .lock()
        .expect("quota refresh lock poisoned");
    coordinator
        .all_states()
        .into_iter()
        .map(|(account_id, quota)| {
            (
                account_id.as_str().to_string(),
                visible_quota_state_value(state.lifecycle.preferences().monitoring_paused, quota),
            )
        })
        .collect()
}

fn visible_quota_state_value(paused: bool, current: QuotaState) -> QuotaState {
    if paused {
        match current {
            QuotaState::Error {
                reason: crate::quota::QuotaErrorReason::Paused,
                ..
            } => current,
            other => QuotaState::Error {
                reason: crate::quota::QuotaErrorReason::Paused,
                last_snapshot: other.snapshot().cloned(),
                failed_at: Utc::now(),
                retry_at: None,
            },
        }
    } else {
        current
    }
}

/// Pin preference only — does not switch Codex login or rewrite auth.json.
#[tauri::command]
pub(crate) fn activate_account(
    account_key: String,
    state: State<'_, AppState>,
) -> Result<DiscoveredAccount, String> {
    let mut accounts = state.credentials.discover_accounts();
    if let Some(snapshot) = state.quota.latest().snapshot() {
        accounts.insert(
            0,
            DiscoveredAccount {
                account_key: crate::quota::CURRENT_CODEX_ACCOUNT_ID.to_string(),
                display_name: snapshot.account.display_name.clone(),
                auth_source: "active".to_string(),
                is_managed: false,
                status: None,
                pinned: false,
            },
        );
    }
    let account = accounts
        .into_iter()
        .find(|account| account.account_key == account_key)
        .ok_or_else(|| format!("Account '{account_key}' not found"))?;
    let _ = state.lifecycle.set_active_account(Some(&account_key))?;
    if account.is_managed {
        let _ = state.credentials.set_pinned(&account_key, true);
    }
    Ok(account)
}

#[tauri::command]
pub(crate) fn refresh_account(
    account_key: String,
    state: State<'_, AppState>,
) -> Result<QuotaState, String> {
    if state.lifecycle.preferences().monitoring_paused {
        return Ok(visible_quota_state(true, &state.quota));
    }
    let coordinator = state
        .quota_refresh
        .lock()
        .expect("quota refresh lock poisoned");
    coordinator
        .manual_refresh_account(&account_key.as_str().into())
        .ok_or_else(|| format!("Account '{account_key}' is not tracked"))
}

#[tauri::command]
pub(crate) fn refresh_quotas(state: State<'_, AppState>) -> Vec<(String, QuotaState)> {
    if !state.lifecycle.preferences().monitoring_paused {
        let coordinator = state
            .quota_refresh
            .lock()
            .expect("quota refresh lock poisoned");
        coordinator.refresh_all();
    }
    get_all_quotas(state)
}

#[tauri::command]
pub(crate) fn remove_account(
    account_key: String,
    delete_history: bool,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let mode = if delete_history {
        crate::credentials::DeleteMode::CredentialsAndHistory
    } else {
        crate::credentials::DeleteMode::CredentialsOnly
    };
    state.credentials.delete_account(&account_key, mode)?;
    if delete_history {
        let _ = state.governance.delete_account_history(&account_key)?;
    }
    let mut coordinator = state
        .quota_refresh
        .lock()
        .expect("quota refresh lock poisoned");
    coordinator.remove_account(&account_key.as_str().into());
    Ok(())
}

#[tauri::command]
pub(crate) fn set_account_alias(
    account_key: String,
    alias: String,
    state: State<'_, AppState>,
) -> Result<DiscoveredAccount, String> {
    let record = state.credentials.set_alias(&account_key, &alias)?;
    Ok(DiscoveredAccount {
        account_key: record.account_id,
        display_name: record.alias,
        auth_source: "managed".to_string(),
        is_managed: true,
        status: Some(record.status),
        pinned: record.pinned,
    })
}

/// Start Codex OAuth PKCE; tokens stay in Keychain/memory. Returns secret-free DTO fields only.
#[tauri::command]
pub(crate) fn start_codex_login(
    state: State<'_, AppState>,
) -> Result<crate::oauth::OAuthResultDto, String> {
    let pending = state
        .credentials
        .begin_pending_account("")
        .map_err(|error| error.to_string())?;
    let tokens = crate::oauth::run_codex_oauth_login(None)
        .map_err(|error| format!("Codex OAuth login failed: {error}"))?;
    let alias = if tokens.email == "unknown" {
        format!("Account · {}", &pending.account_id[..8])
    } else {
        tokens.email.clone()
    };
    let record = state
        .credentials
        .complete_authorization(
            &pending.account_id,
            &tokens.account_id,
            &alias,
            "unknown",
            &tokens.refresh_token,
        )
        .map_err(|error| {
            let _ = state.credentials.delete_account(
                &pending.account_id,
                crate::credentials::DeleteMode::CredentialsAndHistory,
            );
            error
        })?;

    let source = std::sync::Arc::new(crate::quota_token::DirectHttpsQuotaSource::new(
        record.account_id.clone(),
        record.identity_fingerprint.clone(),
        record.alias.clone(),
        state.credentials.clone(),
    ));
    let service = std::sync::Arc::new(QuotaService::with_store(
        record.account_id.clone(),
        source,
        std::sync::Arc::new(state.governance.database().clone()),
    ));
    {
        let mut coordinator = state
            .quota_refresh
            .lock()
            .expect("quota refresh lock poisoned");
        coordinator.add_account(service.clone());
    }
    let _ = service.manual_refresh();

    Ok(crate::oauth::OAuthResultDto {
        account_id: record.account_id,
        alias: record.alias,
        identity_fingerprint: record.identity_fingerprint,
        status: "active".to_string(),
    })
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use chrono::Utc;

    use super::*;
    use crate::quota::{
        QuotaAccount, QuotaFailureKind, QuotaRefreshError, QuotaSnapshot, QuotaSource,
    };
    use crate::{
        database::Database,
        token_usage::{TokenUsageFilters, TokenUsageService},
    };

    struct NoopNotificationSender;

    impl crate::notification::NotificationSender for NoopNotificationSender {
        fn send(
            &self,
            _notification: &crate::notification::SystemNotification,
        ) -> Result<(), String> {
            Ok(())
        }
    }

    struct CountingSource {
        calls: AtomicUsize,
        result: Result<QuotaSnapshot, QuotaRefreshError>,
    }

    #[test]
    fn boolean_system_preference_rolls_back_when_persistence_fails() {
        let synchronized_values = RefCell::new(Vec::new());

        let result = sync_and_persist_boolean_preference(
            false,
            true,
            |value| {
                synchronized_values.borrow_mut().push(value);
                Ok(())
            },
            |_| Err("save failed".to_string()),
        );

        assert_eq!(result, Err("save failed".to_string()));
        assert_eq!(synchronized_values.into_inner(), vec![true, false]);
    }

    impl QuotaSource for CountingSource {
        fn refresh(&self) -> Result<QuotaSnapshot, QuotaRefreshError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.result.clone()
        }
    }

    #[test]
    fn refresh_ipc_obeys_pause_without_contacting_the_account() {
        let source = Arc::new(CountingSource {
            calls: AtomicUsize::new(0),
            result: Ok(QuotaSnapshot {
                account: QuotaAccount {
                    id: "account-1".into(),
                    display_name: "user@example.com".to_string(),
                    plan_type: "plus".to_string(),
                },
                windows: vec![],
                updated_at: Utc::now(),
            }),
        });
        let service = QuotaService::new("account-1", source.clone());

        let state = refresh_quota_service(true, &service);

        assert!(matches!(
            state,
            QuotaState::Error {
                reason: crate::quota::QuotaErrorReason::Paused,
                ..
            }
        ));
        assert_eq!(source.calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn get_quota_ipc_keeps_paused_state_visible() {
        let source = Arc::new(CountingSource {
            calls: AtomicUsize::new(0),
            result: Ok(QuotaSnapshot {
                account: QuotaAccount {
                    id: "account-1".into(),
                    display_name: "user@example.com".to_string(),
                    plan_type: "plus".to_string(),
                },
                windows: vec![],
                updated_at: Utc::now(),
            }),
        });
        let service = QuotaService::new("account-1", source);

        assert!(matches!(
            visible_quota_state(true, &service),
            QuotaState::Error {
                reason: crate::quota::QuotaErrorReason::Paused,
                ..
            }
        ));
    }

    #[test]
    fn refresh_ipc_exposes_recovery_state_but_not_upstream_error_details() {
        let source = Arc::new(CountingSource {
            calls: AtomicUsize::new(0),
            result: Err(QuotaRefreshError::new(
                QuotaFailureKind::Transport,
                "secret diagnostic token abc123",
            )),
        });
        let service = QuotaService::new("account-1", source);

        let state = refresh_quota_service(false, &service);
        let dto = serde_json::to_string(&state).unwrap();

        assert!(dto.contains("\"reason\":\"transport\""));
        assert!(!dto.contains("secret diagnostic"));
        assert!(!dto.contains("abc123"));
    }

    #[test]
    fn refresh_token_ipc_obeys_pause_without_ingesting_session_files() {
        let database = Database::in_memory().unwrap();
        let fixture_root =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/token");
        let service = TokenUsageService::new(database.clone(), vec![fixture_root]);

        let state = refresh_token_usage_service(true, &service, TokenUsageFilters::default());

        assert!(matches!(
            state,
            TokenUsageState::Stale {
                reason: crate::token_usage::TokenUsageStaleReason::Paused,
                ..
            }
        ));
        database
            .with_connection(|connection| {
                let count: i64 = connection
                    .query_row("SELECT COUNT(*) FROM token_usage_events", [], |row| {
                        row.get(0)
                    })
                    .map_err(|error| error.to_string())?;
                assert_eq!(count, 0);
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn notification_commands_round_trip_through_the_tauri_ipc_boundary() {
        let database = crate::database::Database::in_memory().unwrap();
        let lifecycle =
            Arc::new(crate::lifecycle::LifecycleService::new(Arc::new(database.clone())).unwrap());
        let notifications = Arc::new(
            crate::notification::NotificationService::new(
                Arc::new(database),
                Arc::new(NoopNotificationSender),
            )
            .unwrap(),
        );
        let app = tauri::test::mock_builder()
            .manage(NotificationIpcState {
                lifecycle,
                notifications,
            })
            .invoke_handler(tauri::generate_handler![
                get_notification_status,
                set_notification_preferences
            ])
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
            .build()
            .unwrap();
        let response = tauri::test::get_ipc_response(
            &webview,
            tauri::webview::InvokeRequest {
                cmd: "set_notification_preferences".into(),
                callback: tauri::ipc::CallbackFn(0),
                error: tauri::ipc::CallbackFn(1),
                url: "tauri://localhost".parse().unwrap(),
                body: tauri::ipc::InvokeBody::Json(serde_json::json!({
                    "notifications": {
                        "enabled": true,
                        "quotaThresholds": [25, 5, 0],
                        "diskAvailablePercentThreshold": 12,
                        "consecutiveRefreshFailures": 4
                    }
                })),
                headers: Default::default(),
                invoke_key: tauri::test::INVOKE_KEY.to_string(),
            },
        )
        .unwrap()
        .deserialize::<serde_json::Value>()
        .unwrap();
        assert_eq!(
            response["notifications"]["quotaThresholds"],
            serde_json::json!([25, 5, 0])
        );

        let status = tauri::test::get_ipc_response(
            &webview,
            tauri::webview::InvokeRequest {
                cmd: "get_notification_status".into(),
                callback: tauri::ipc::CallbackFn(2),
                error: tauri::ipc::CallbackFn(3),
                url: "tauri://localhost".parse().unwrap(),
                body: tauri::ipc::InvokeBody::default(),
                headers: Default::default(),
                invoke_key: tauri::test::INVOKE_KEY.to_string(),
            },
        )
        .unwrap()
        .deserialize::<serde_json::Value>()
        .unwrap();
        assert_eq!(status["activeConditions"], serde_json::json!([]));
        assert_eq!(status["deliveryError"], serde_json::Value::Null);
    }
}
