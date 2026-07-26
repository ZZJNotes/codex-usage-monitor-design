mod commands;
pub mod database;
pub mod lifecycle;
pub mod platform_metrics;
pub mod system_health;
mod tray;

use std::{
    fs,
    sync::{Arc, RwLock},
    thread,
    time::Duration,
};

use commands::{
    get_application_status, get_lifecycle_preferences, get_system_health,
    get_system_health_history, set_locale, set_monitoring_paused, set_theme, show_dashboard,
};
use database::Database;
use lifecycle::LifecycleService;
use platform_metrics::MacMetricSource;
use serde::Serialize;
use system_health::{SystemHealthService, SystemHealthState};
use tauri::{ActivationPolicy, AppHandle, Manager, Runtime, WindowEvent};
use tray::setup_tray;

pub(crate) struct AppState {
    pub(crate) health: Arc<SystemHealthService>,
    pub(crate) lifecycle: Arc<LifecycleService>,
    pub(crate) application_status: Arc<RwLock<ApplicationStatus>>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ApplicationStatus {
    pub(crate) storage_error: Option<String>,
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
            app.manage(setup_tray(
                app.handle(),
                preferences.locale,
                preferences.monitoring_paused,
            )?);

            #[cfg(target_os = "macos")]
            app.set_activation_policy(ActivationPolicy::Accessory);

            thread::spawn(move || loop {
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
