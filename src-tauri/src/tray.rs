use tauri::{
    AppHandle, Manager, PhysicalPosition, Rect, WebviewUrl, WebviewWindowBuilder,
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
};

use crate::tray_view::build_tray_view;

pub(crate) fn os_locale() -> String {
    sys_locale::get_locale().unwrap_or_else(|| "en-US".to_string())
}

fn should_toggle_popover(button: MouseButton, button_state: MouseButtonState) -> bool {
    button == MouseButton::Left && button_state == MouseButtonState::Up
}

fn popover_position(icon_rect: Rect, popover_width: u32) -> PhysicalPosition<i32> {
    let icon_position = icon_rect.position.to_physical::<f64>(1.0);
    let icon_size = icon_rect.size.to_physical::<f64>(1.0);
    let x = icon_position.x + (icon_size.width - f64::from(popover_width)) / 2.0;
    let y = icon_position.y + icon_size.height;
    PhysicalPosition::new(x.round() as i32, y.round() as i32)
}

pub(crate) fn toggle_popover(app: &AppHandle, icon_rect: Rect) {
    if let Some(window) = app.get_webview_window("popover") {
        if window.is_visible().unwrap_or(false) {
            let _ = window.hide();
        } else {
            if let Ok(size) = window.outer_size() {
                let _ = window.set_position(popover_position(icon_rect, size.width));
            }
            let _ = window.show();
            let _ = window.set_focus();
        }
    }
}

pub(crate) fn setup_tray(app: &AppHandle) -> tauri::Result<()> {
    // Create popover window upfront (hidden) so it's ready when the user clicks
    WebviewWindowBuilder::new(app, "popover", WebviewUrl::App("index.html#popover".into()))
        .title("")
        .inner_size(420.0, 520.0)
        .decorations(false)
        .always_on_top(true)
        .visible(false)
        .build()?;

    TrayIconBuilder::with_id("main-tray")
        .tooltip("Codex Usage Monitor")
        .on_tray_icon_event(|tray, event| match event {
            TrayIconEvent::Click {
                button,
                button_state,
                rect,
                ..
            } if should_toggle_popover(button, button_state) => {
                toggle_popover(tray.app_handle(), rect);
            }
            _ => {}
        })
        .build(app)?;
    Ok(())
}

pub(crate) fn update_tray_title(app: &AppHandle) {
    let state = app.state::<crate::AppState>();
    let preferences = state.lifecycle.preferences();
    let health = state.health.latest();
    let quota = state.quota.latest();
    let view = build_tray_view(&preferences, &health, &quota, &os_locale());
    if let Some(tray_icon) = app.tray_by_id("main-tray") {
        let _ = tray_icon.set_title(Some(&view.title));
        let _ = tray_icon.set_tooltip(Some(&view.status));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_physical_left_click_toggles_the_popover_once() {
        let toggle_count = [MouseButtonState::Down, MouseButtonState::Up]
            .into_iter()
            .filter(|state| should_toggle_popover(MouseButton::Left, *state))
            .count();

        assert_eq!(toggle_count, 1);
    }

    #[test]
    fn popover_is_centered_below_the_tray_icon() {
        let icon_rect = Rect {
            position: tauri::PhysicalPosition::new(960, 0).into(),
            size: tauri::PhysicalSize::new(44, 24).into(),
        };

        assert_eq!(
            popover_position(icon_rect, 420),
            PhysicalPosition::new(772, 24)
        );
    }
}
