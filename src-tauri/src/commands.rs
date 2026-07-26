use chrono::Utc;
use tauri::{AppHandle, State};

use crate::{
    AppState, ApplicationStatus,
    lifecycle::LifecyclePreferences,
    show_main_window,
    system_health::{SystemHealthPoint, SystemHealthState},
    tray::{TrayMenuItems, update_tray_locale, update_tray_text},
};

#[tauri::command]
pub(crate) fn get_system_health(state: State<'_, AppState>) -> SystemHealthState {
    let paused = state.lifecycle.preferences().monitoring_paused;
    match state.health.latest() {
        SystemHealthState::Ready {
            updated_at,
            metrics,
        } if paused || (Utc::now() - updated_at).num_seconds() > 10 => SystemHealthState::Stale {
            updated_at,
            metrics,
            reason: if paused { "paused" } else { "outdated" }.to_string(),
        },
        current => current,
    }
}

#[tauri::command]
pub(crate) fn get_system_health_history(state: State<'_, AppState>) -> Vec<SystemHealthPoint> {
    state.health.history()
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
