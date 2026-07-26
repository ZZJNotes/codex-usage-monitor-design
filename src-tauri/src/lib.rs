pub mod database;
pub mod lifecycle;
pub mod platform_metrics;
pub mod system_health;

use std::{
    fs,
    sync::{Arc, RwLock},
    thread,
    time::Duration,
};

use database::Database;
use lifecycle::{LifecycleService, Locale};
use platform_metrics::MacMetricSource;
use serde::Serialize;
use system_health::{SystemHealthPoint, SystemHealthService, SystemHealthState};
use tauri::{
    ActivationPolicy, AppHandle, Manager, Runtime, State, WindowEvent,
    image::Image,
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::TrayIconBuilder,
};

struct AppState {
    health: Arc<SystemHealthService>,
    lifecycle: Arc<LifecycleService>,
    application_status: Arc<RwLock<ApplicationStatus>>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ApplicationStatus {
    storage_error: Option<String>,
}

struct TrayMenuItems {
    open: MenuItem<tauri::Wry>,
    pause: MenuItem<tauri::Wry>,
    quit: MenuItem<tauri::Wry>,
}

#[tauri::command]
fn get_system_health(state: State<'_, AppState>) -> SystemHealthState {
    state.health.latest()
}

#[tauri::command]
fn get_system_health_history(state: State<'_, AppState>) -> Vec<SystemHealthPoint> {
    state.health.history()
}

#[tauri::command]
fn get_application_status(state: State<'_, AppState>) -> ApplicationStatus {
    state
        .application_status
        .read()
        .expect("application status poisoned")
        .clone()
}

#[tauri::command]
fn get_lifecycle_preferences(state: State<'_, AppState>) -> lifecycle::LifecyclePreferences {
    state.lifecycle.preferences()
}

#[tauri::command]
fn set_monitoring_paused(
    paused: bool,
    state: State<'_, AppState>,
) -> Result<lifecycle::LifecyclePreferences, String> {
    state.lifecycle.set_monitoring_paused(paused)
}

#[tauri::command]
fn set_theme(
    theme: String,
    state: State<'_, AppState>,
) -> Result<lifecycle::LifecyclePreferences, String> {
    state.lifecycle.set_theme(&theme)
}

#[tauri::command]
fn set_locale(
    locale: String,
    state: State<'_, AppState>,
    tray: State<'_, TrayMenuItems>,
    app: AppHandle,
) -> Result<lifecycle::LifecyclePreferences, String> {
    let preferences = state.lifecycle.set_locale(&locale)?;
    update_tray_text(&tray, preferences.locale, preferences.monitoring_paused);
    let (_, _, _, tooltip) = tray_text(preferences.locale, preferences.monitoring_paused);
    if let Some(tray_icon) = app.tray_by_id("main-tray") {
        let _ = tray_icon.set_tooltip(Some(tooltip));
    }
    Ok(preferences)
}

#[tauri::command]
fn show_dashboard(app: AppHandle) -> Result<(), String> {
    show_main_window(&app)
}

fn show_main_window<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "dashboard window is unavailable".to_string())?;
    window.show().map_err(|error| error.to_string())?;
    window.unminimize().map_err(|error| error.to_string())?;
    window.set_focus().map_err(|error| error.to_string())
}

fn tray_text(
    locale: Locale,
    paused: bool,
) -> (&'static str, &'static str, &'static str, &'static str) {
    match (locale, paused) {
        (Locale::ZhCn, false) => ("打开仪表盘", "暂停监控", "退出", "Codex 用量监控"),
        (Locale::ZhCn, true) => ("打开仪表盘", "恢复监控", "退出", "Codex 用量监控"),
        (Locale::En, false) => (
            "Open dashboard",
            "Pause monitoring",
            "Quit",
            "Codex Usage Monitor",
        ),
        (Locale::En, true) => (
            "Open dashboard",
            "Resume monitoring",
            "Quit",
            "Codex Usage Monitor",
        ),
    }
}

fn update_tray_text(items: &TrayMenuItems, locale: Locale, paused: bool) {
    let (open, pause, quit, _) = tray_text(locale, paused);
    let _ = items.open.set_text(open);
    let _ = items.pause.set_text(pause);
    let _ = items.quit.set_text(quit);
}

fn setup_tray(app: &AppHandle, locale: Locale, paused: bool) -> tauri::Result<TrayMenuItems> {
    let (open_text, pause_text, quit_text, tooltip) = tray_text(locale, paused);
    let open = MenuItem::with_id(app, "open", open_text, true, None::<&str>)?;
    let pause = MenuItem::with_id(app, "toggle-pause", pause_text, true, None::<&str>)?;
    let separator = PredefinedMenuItem::separator(app)?;
    let quit = MenuItem::with_id(app, "quit", quit_text, true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &pause, &separator, &quit])?;
    let icon = Image::from_bytes(include_bytes!("../icons/icon.png"))?;

    TrayIconBuilder::with_id("main-tray")
        .icon(icon)
        .icon_as_template(false)
        .tooltip(tooltip)
        .menu(&menu)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "open" => {
                let _ = show_main_window(app);
            }
            "toggle-pause" => {
                let state = app.state::<AppState>();
                let paused = !state.lifecycle.preferences().monitoring_paused;
                if let Ok(preferences) = state.lifecycle.set_monitoring_paused(paused) {
                    let tray = app.state::<TrayMenuItems>();
                    update_tray_text(&tray, preferences.locale, preferences.monitoring_paused);
                }
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .build(app)?;
    Ok(TrayMenuItems { open, pause, quit })
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            get_system_health,
            get_system_health_history,
            get_application_status,
            get_lifecycle_preferences,
            set_monitoring_paused,
            set_theme,
            set_locale,
            show_dashboard,
        ])
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            let database_result = fs::create_dir_all(&data_dir)
                .map_err(|error| error.to_string())
                .and_then(|_| Database::open(&data_dir.join("monitor.sqlite3")));
            let (database, storage_error, ephemeral_storage) = match database_result {
                Ok(database) => (database, None, false),
                Err(error) => (
                    Database::in_memory().map_err(std::io::Error::other)?,
                    Some(format!(
                        "本地数据库不可用：{error}。统计将不会持久保存；请检查应用数据目录权限后重启。"
                    )),
                    true,
                ),
            };
            let lifecycle = Arc::new(
                LifecycleService::new(Arc::new(database.clone())).map_err(std::io::Error::other)?,
            );
            let health = Arc::new(SystemHealthService::new(Arc::new(MacMetricSource::new())));
            let application_status = Arc::new(RwLock::new(ApplicationStatus { storage_error }));
            app.manage(AppState {
                health: health.clone(),
                lifecycle: lifecycle.clone(),
                application_status: application_status.clone(),
            });
            let preferences = lifecycle.preferences();
            let tray_items = setup_tray(
                app.handle(),
                preferences.locale,
                preferences.monitoring_paused,
            )?;
            app.manage(tray_items);

            #[cfg(target_os = "macos")]
            app.set_activation_policy(ActivationPolicy::Accessory);

            thread::spawn(move || {
                loop {
                    if let Ok(Some(SystemHealthState::Ready {
                        updated_at,
                        metrics,
                    })) = lifecycle.sample_if_active(&health)
                    {
                        match database.record_health_metrics(updated_at, &metrics) {
                            Ok(()) if !ephemeral_storage => {
                                application_status
                                    .write()
                                    .expect("application status poisoned")
                                    .storage_error = None;
                            }
                            Err(error) => {
                                let message = format!(
                                    "系统指标无法保存：{error}。请检查应用数据目录权限后重试。"
                                );
                                application_status
                                    .write()
                                    .expect("application status poisoned")
                                    .storage_error = Some(message.clone());
                                health.report_error(message);
                            }
                            _ => {}
                        }
                    }
                    thread::sleep(Duration::from_secs(2));
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
mod tests {
    use super::*;

    #[test]
    fn tray_actions_follow_the_saved_locale_and_pause_state() {
        assert_eq!(
            tray_text(Locale::En, true),
            (
                "Open dashboard",
                "Resume monitoring",
                "Quit",
                "Codex Usage Monitor"
            )
        );
        assert_eq!(tray_text(Locale::ZhCn, false).1, "暂停监控");
    }
}
