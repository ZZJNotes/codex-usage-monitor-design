pub mod database;
pub mod lifecycle;
pub mod platform_metrics;
pub mod system_health;

use std::{fs, sync::Arc, thread, time::Duration};

use database::Database;
use lifecycle::LifecycleService;
use platform_metrics::MacMetricSource;
use system_health::{SystemHealthService, SystemHealthState};
use tauri::{
    ActivationPolicy, AppHandle, Manager, Runtime, State, WindowEvent,
    image::Image,
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::TrayIconBuilder,
};

struct AppState {
    health: Arc<SystemHealthService>,
    lifecycle: Arc<LifecycleService>,
}

#[tauri::command]
fn get_system_health(state: State<'_, AppState>) -> SystemHealthState {
    state.health.latest()
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
) -> Result<lifecycle::LifecyclePreferences, String> {
    state.lifecycle.set_locale(&locale)
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

fn setup_tray(app: &AppHandle) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "打开仪表盘", true, None::<&str>)?;
    let pause = MenuItem::with_id(app, "toggle-pause", "暂停 / 恢复监控", true, None::<&str>)?;
    let separator = PredefinedMenuItem::separator(app)?;
    let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &pause, &separator, &quit])?;
    let icon = Image::from_bytes(include_bytes!("../icons/icon.png"))?;

    TrayIconBuilder::with_id("main-tray")
        .icon(icon)
        .icon_as_template(true)
        .tooltip("Codex 用量监控")
        .menu(&menu)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "open" => {
                let _ = show_main_window(app);
            }
            "toggle-pause" => {
                let state = app.state::<AppState>();
                let paused = !state.lifecycle.preferences().monitoring_paused;
                let _ = state.lifecycle.set_monitoring_paused(paused);
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .build(app)?;
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            get_system_health,
            get_lifecycle_preferences,
            set_monitoring_paused,
            set_theme,
            set_locale,
            show_dashboard,
        ])
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            fs::create_dir_all(&data_dir)?;
            let database =
                Database::open(&data_dir.join("monitor.sqlite3")).map_err(std::io::Error::other)?;
            let lifecycle = Arc::new(
                LifecycleService::new(Arc::new(database.clone())).map_err(std::io::Error::other)?,
            );
            let health = Arc::new(SystemHealthService::new(Arc::new(MacMetricSource::new())));
            app.manage(AppState {
                health: health.clone(),
                lifecycle: lifecycle.clone(),
            });
            setup_tray(app.handle())?;

            #[cfg(target_os = "macos")]
            app.set_activation_policy(ActivationPolicy::Accessory);

            thread::spawn(move || {
                loop {
                    if let Ok(Some(SystemHealthState::Ready {
                        updated_at,
                        metrics,
                    })) = lifecycle.sample_if_active(&health)
                    {
                        let _ = database.record_health_metrics(updated_at, &metrics);
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
