use tauri::{
    AppHandle, Manager,
    image::Image,
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::TrayIconBuilder,
};

use crate::{
    AppState,
    lifecycle::LifecyclePreferences,
    show_main_window,
    tray_view::{build_tray_view, pinned_quota_available, tray_copy},
};

pub(crate) struct TrayMenuItems {
    status: MenuItem<tauri::Wry>,
    updated: MenuItem<tauri::Wry>,
    reset: MenuItem<tauri::Wry>,
    refresh: MenuItem<tauri::Wry>,
    open: MenuItem<tauri::Wry>,
    pause: MenuItem<tauri::Wry>,
    quit: MenuItem<tauri::Wry>,
}

fn actions(
    preferences: &LifecyclePreferences,
    paused: bool,
    pinned: bool,
) -> (&'static str, &'static str, &'static str, &'static str) {
    let copy = tray_copy(preferences.locale);
    let refresh = if paused {
        copy.refresh_generic
    } else if pinned {
        copy.refresh_pinned
    } else {
        copy.refresh_current
    };
    (
        refresh,
        copy.open,
        if paused { copy.resume } else { copy.pause },
        copy.quit,
    )
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
    let (refresh, open, pause, quit) = actions(&preferences, preferences.monitoring_paused, pinned);
    let _ = items.status.set_text(&view.status);
    let _ = items.updated.set_text(&view.updated);
    let _ = items.reset.set_text(&view.reset);
    let _ = items.refresh.set_text(refresh);
    let _ = items.refresh.set_enabled(
        !preferences.monitoring_paused && pinned_quota_available(&preferences, &quota),
    );
    let _ = items.open.set_text(open);
    let _ = items.pause.set_text(pause);
    let _ = items.quit.set_text(quit);
    if let Some(tray_icon) = app.tray_by_id("main-tray") {
        let _ = tray_icon.set_title(Some(&view.title));
        let _ = tray_icon.set_tooltip(Some(tray_copy(preferences.locale).tooltip));
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
        actions(preferences, preferences.monitoring_paused, pinned);
    let status = MenuItem::with_id(app, "summary-status", view.status, false, None::<&str>)?;
    let updated = MenuItem::with_id(app, "summary-updated", view.updated, false, None::<&str>)?;
    let reset = MenuItem::with_id(app, "summary-reset", view.reset, false, None::<&str>)?;
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
            &status, &updated, &reset, &separator, &refresh, &open, &pause, &separator, &quit,
        ],
    )?;
    let icon = Image::from_bytes(include_bytes!("../icons/icon.png"))?;

    TrayIconBuilder::with_id("main-tray")
        .icon(icon)
        .icon_as_template(false)
        .title(&view.title)
        .tooltip(tray_copy(preferences.locale).tooltip)
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
        reset,
        refresh,
        open,
        pause,
        quit,
    })
}
