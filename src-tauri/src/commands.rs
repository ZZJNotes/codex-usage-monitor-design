use chrono::Utc;
use tauri::{AppHandle, State};

use crate::{
    AppState, ApplicationStatus,
    lifecycle::LifecyclePreferences,
    quota::QuotaState,
    show_main_window,
    system_health::{StaleReason, SystemHealthPoint, SystemHealthState},
    token_usage::{TokenUsageFilters, TokenUsageState},
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
    state.quota.latest()
}

#[tauri::command]
pub(crate) fn refresh_quota(state: State<'_, AppState>) -> QuotaState {
    if state.lifecycle.preferences().monitoring_paused {
        return state.quota.paused();
    }
    state.quota.refresh()
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
    if state.lifecycle.preferences().monitoring_paused {
        return state.token_usage.query(filters).paused();
    }
    let _ = state.token_usage.scan();
    state.token_usage.query(filters)
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
