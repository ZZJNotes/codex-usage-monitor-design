use tauri::{
    AppHandle, Manager,
    image::Image,
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::TrayIconBuilder,
};

use crate::{AppState, lifecycle::Locale, show_main_window};

pub(crate) struct TrayMenuItems {
    open: MenuItem<tauri::Wry>,
    pause: MenuItem<tauri::Wry>,
    quit: MenuItem<tauri::Wry>,
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

pub(crate) fn update_tray_text(items: &TrayMenuItems, locale: Locale, paused: bool) {
    let (open, pause, quit, _) = tray_text(locale, paused);
    let _ = items.open.set_text(open);
    let _ = items.pause.set_text(pause);
    let _ = items.quit.set_text(quit);
}

pub(crate) fn update_tray_locale(
    app: &AppHandle,
    items: &TrayMenuItems,
    locale: Locale,
    paused: bool,
) {
    update_tray_text(items, locale, paused);
    let (_, _, _, tooltip) = tray_text(locale, paused);
    if let Some(tray_icon) = app.tray_by_id("main-tray") {
        let _ = tray_icon.set_tooltip(Some(tooltip));
    }
}

pub(crate) fn setup_tray(
    app: &AppHandle,
    locale: Locale,
    paused: bool,
) -> tauri::Result<TrayMenuItems> {
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
