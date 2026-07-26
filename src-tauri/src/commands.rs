use chrono::Utc;
use tauri::{AppHandle, State};

use crate::{
    AppState, ApplicationStatus,
    lifecycle::LifecyclePreferences,
    quota::{QuotaService, QuotaState},
    show_main_window,
    system_health::{StaleReason, SystemHealthPoint, SystemHealthState},
    tray::{TrayMenuItems, update_tray_locale, update_tray_text},
};

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
pub(crate) fn refresh_quota(state: State<'_, AppState>) -> QuotaState {
    refresh_quota_service(
        state.lifecycle.preferences().monitoring_paused,
        &state.quota,
    )
}

#[tauri::command]
pub(crate) fn recover_quota(state: State<'_, AppState>) -> QuotaState {
    recover_quota_service(
        state.lifecycle.preferences().monitoring_paused,
        &state.quota,
    )
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
pub(crate) fn set_monitoring_paused(
    paused: bool,
    state: State<'_, AppState>,
    tray: State<'_, TrayMenuItems>,
) -> Result<LifecyclePreferences, String> {
    let preferences = state.lifecycle.set_monitoring_paused(paused)?;
    update_tray_text(&tray, preferences.locale, preferences.monitoring_paused);
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
pub(crate) fn set_locale(
    locale: String,
    state: State<'_, AppState>,
    tray: State<'_, TrayMenuItems>,
    app: AppHandle,
) -> Result<LifecyclePreferences, String> {
    let preferences = state.lifecycle.set_locale(&locale)?;
    update_tray_locale(
        &app,
        &tray,
        preferences.locale,
        preferences.monitoring_paused,
    );
    Ok(preferences)
}

#[tauri::command]
pub(crate) fn show_dashboard(app: AppHandle) -> Result<(), String> {
    show_main_window(&app)
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use chrono::Utc;

    use super::*;
    use crate::quota::{
        QuotaAccount, QuotaFailureKind, QuotaRefreshError, QuotaSnapshot, QuotaSource,
    };

    struct CountingSource {
        calls: AtomicUsize,
        result: Result<QuotaSnapshot, QuotaRefreshError>,
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
                    id: "account-1".to_string(),
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
                    id: "account-1".to_string(),
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
}
