use tauri::{
    AppHandle, Manager,
    image::Image,
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::TrayIconBuilder,
};

use crate::{
    AppState,
    lifecycle::{LifecyclePreferences, Locale},
    show_main_window,
    tray_view::{build_tray_view, pinned_quota_available},
};

pub(crate) struct TrayMenuItems {
    status: MenuItem<tauri::Wry>,
    updated: MenuItem<tauri::Wry>,
    refresh: MenuItem<tauri::Wry>,
    open: MenuItem<tauri::Wry>,
    pause: MenuItem<tauri::Wry>,
    quit: MenuItem<tauri::Wry>,
}

fn actions(
    locale: Locale,
    paused: bool,
    pinned: bool,
) -> (&'static str, &'static str, &'static str, &'static str) {
    match (locale, paused, pinned) {
        (Locale::ZhCn, false, false) => ("刷新当前账户", "打开仪表盘", "暂停监控", "退出"),
        (Locale::ZhCn, false, true) => ("刷新置顶账户", "打开仪表盘", "暂停监控", "退出"),
        (Locale::ZhCn, true, _) => ("刷新额度", "打开仪表盘", "恢复监控", "退出"),
        (Locale::En, false, false) => (
            "Refresh current account",
            "Open dashboard",
            "Pause monitoring",
            "Quit",
        ),
        (Locale::En, false, true) => (
            "Refresh pinned account",
            "Open dashboard",
            "Pause monitoring",
            "Quit",
        ),
        (Locale::En, true, _) => (
            "Refresh quota",
            "Open dashboard",
            "Resume monitoring",
            "Quit",
        ),
    }
}

fn os_locale() -> String {
    sys_locale::get_locale().unwrap_or_else(|| "en-US".to_string())
}

pub(crate) fn update_tray(app: &AppHandle, items: &TrayMenuItems) {
    let state = app.state::<AppState>();
    let preferences = state.lifecycle.preferences();
    let health = state.health.latest();
    let quota = state.quota.latest();
    let view = build_tray_view(&preferences, &health, &quota, &os_locale());
    let pinned = preferences.menu_bar.pinned_account_id.is_some();
    let (refresh, open, pause, quit) =
        actions(preferences.locale, preferences.monitoring_paused, pinned);
    let _ = items.status.set_text(&view.status);
    let _ = items.updated.set_text(&view.updated);
    let _ = items.refresh.set_text(refresh);
    let _ = items.refresh.set_enabled(
        !preferences.monitoring_paused && pinned_quota_available(&preferences, &quota),
    );
    let _ = items.open.set_text(open);
    let _ = items.pause.set_text(pause);
    let _ = items.quit.set_text(quit);
    if let Some(tray_icon) = app.tray_by_id("main-tray") {
        let _ = tray_icon.set_title(Some(&view.title));
        let _ = tray_icon.set_tooltip(Some(if preferences.locale == Locale::ZhCn {
            "Codex 用量监控"
        } else {
            "Codex Usage Monitor"
        }));
    }
}

pub(crate) fn setup_tray(
    app: &AppHandle,
    preferences: &LifecyclePreferences,
) -> tauri::Result<TrayMenuItems> {
    let view = build_tray_view(
        preferences,
        &crate::system_health::SystemHealthState::Loading,
        &crate::quota::QuotaState::Loading,
        &os_locale(),
    );
    let pinned = preferences.menu_bar.pinned_account_id.is_some();
    let (refresh_text, open_text, pause_text, quit_text) =
        actions(preferences.locale, preferences.monitoring_paused, pinned);
    let status = MenuItem::with_id(app, "summary-status", view.status, false, None::<&str>)?;
    let updated = MenuItem::with_id(app, "summary-updated", view.updated, false, None::<&str>)?;
    let refresh = MenuItem::with_id(
        app,
        "refresh-quota",
        refresh_text,
        !preferences.monitoring_paused && !pinned,
        Some("CmdOrCtrl+R"),
    )?;
    let open = MenuItem::with_id(app, "open", open_text, true, Some("CmdOrCtrl+D"))?;
    let pause = MenuItem::with_id(app, "toggle-pause", pause_text, true, None::<&str>)?;
    let separator = PredefinedMenuItem::separator(app)?;
    let quit = MenuItem::with_id(app, "quit", quit_text, true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[
            &status, &updated, &separator, &refresh, &open, &pause, &separator, &quit,
        ],
    )?;
    let icon = Image::from_bytes(include_bytes!("../icons/icon.png"))?;

    TrayIconBuilder::with_id("main-tray")
        .icon(icon)
        .icon_as_template(false)
        .title(&view.title)
        .tooltip(if preferences.locale == Locale::ZhCn {
            "Codex 用量监控"
        } else {
            "Codex Usage Monitor"
        })
        .menu(&menu)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "refresh-quota" => {
                let state = app.state::<AppState>();
                let preferences = state.lifecycle.preferences();
                if !preferences.monitoring_paused
                    && pinned_quota_available(&preferences, &state.quota.latest())
                {
                    let _ = state.quota.manual_refresh();
                }
                let tray = app.state::<TrayMenuItems>();
                update_tray(app, &tray);
            }
            "open" => {
                let _ = show_main_window(app);
            }
            "toggle-pause" => {
                let state = app.state::<AppState>();
                let paused = !state.lifecycle.preferences().monitoring_paused;
                if state.lifecycle.set_monitoring_paused(paused).is_ok() {
                    let tray = app.state::<TrayMenuItems>();
                    update_tray(app, &tray);
                }
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .build(app)?;
    Ok(TrayMenuItems {
        status,
        updated,
        refresh,
        open,
        pause,
        quit,
    })
}
