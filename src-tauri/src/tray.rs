use chrono::{DateTime, Utc};
use tauri::{
    AppHandle, Manager,
    image::Image,
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::TrayIconBuilder,
};

use crate::{
    AppState,
    lifecycle::{LifecyclePreferences, Locale},
    quota::{QuotaSnapshot, QuotaState},
    show_main_window,
    system_health::{SystemHealthMetrics, SystemHealthState},
};

pub(crate) struct TrayMenuItems {
    status: MenuItem<tauri::Wry>,
    updated: MenuItem<tauri::Wry>,
    refresh: MenuItem<tauri::Wry>,
    open: MenuItem<tauri::Wry>,
    pause: MenuItem<tauri::Wry>,
    quit: MenuItem<tauri::Wry>,
}

#[derive(Debug, PartialEq)]
struct TrayView {
    title: String,
    status: String,
    updated: String,
}

fn actions(
    locale: Locale,
    paused: bool,
) -> (&'static str, &'static str, &'static str, &'static str) {
    match (locale, paused) {
        (Locale::ZhCn, false) => ("刷新置顶账户", "打开仪表盘", "暂停监控", "退出"),
        (Locale::ZhCn, true) => ("刷新置顶账户", "打开仪表盘", "恢复监控", "退出"),
        (Locale::En, false) => (
            "Refresh pinned account",
            "Open dashboard",
            "Pause monitoring",
            "Quit",
        ),
        (Locale::En, true) => (
            "Refresh pinned account",
            "Open dashboard",
            "Resume monitoring",
            "Quit",
        ),
    }
}

fn visible_snapshot<'a>(
    preferences: &LifecyclePreferences,
    quota: &'a QuotaState,
) -> Option<&'a QuotaSnapshot> {
    let snapshot = match quota {
        QuotaState::Ready { snapshot, .. } | QuotaState::Stale { snapshot, .. } => Some(snapshot),
        QuotaState::Error { last_snapshot, .. } => last_snapshot.as_ref(),
        QuotaState::Cooldown { snapshot, .. } => snapshot.as_ref(),
        QuotaState::Loading => None,
    }?;
    preferences
        .menu_bar
        .pinned_account_id
        .as_ref()
        .map(|id| id.as_str())
        .is_none_or(|id| id == snapshot.account.id.as_str())
        .then_some(snapshot)
}

fn current_metrics(health: &SystemHealthState) -> Option<&SystemHealthMetrics> {
    match health {
        SystemHealthState::Ready { metrics, .. } | SystemHealthState::Stale { metrics, .. } => {
            Some(metrics)
        }
        SystemHealthState::Error { last_metrics, .. } => last_metrics.as_ref(),
        SystemHealthState::Loading => None,
    }
}

fn parameter_text(
    id: &str,
    locale: Locale,
    metrics: Option<&SystemHealthMetrics>,
    snapshot: Option<&QuotaSnapshot>,
) -> Option<String> {
    let zh = locale == Locale::ZhCn;
    match id {
        "cpu" => metrics.map(|m| format!("CPU {:.0}%", m.cpu_percent)),
        "memoryPressure" => metrics.map(|m| {
            let state = match (zh, m.memory_pressure.as_str()) {
                (true, "normal") => "正常",
                (true, "warning") => "偏高",
                (true, _) => "严重",
                (false, "normal") => "normal",
                (false, "warning") => "high",
                (false, _) => "critical",
            };
            format!("{} {state}", if zh { "内存" } else { "Memory" })
        }),
        "diskAvailable" => metrics.map(|m| {
            format!(
                "{} {:.0} GB",
                if zh { "磁盘" } else { "Disk" },
                m.disk_available_bytes as f64 / 1_000_000_000.0
            )
        }),
        "networkDown" => metrics.map(|m| {
            format!(
                "↓ {:.1} MB/s",
                m.network_down_bytes_per_second / 1_000_000.0
            )
        }),
        "battery" => metrics
            .and_then(|m| m.battery_percent)
            .map(|value| format!("{} {:.0}%", if zh { "电量" } else { "Battery" }, value)),
        "uptime" => metrics.map(|m| {
            format!(
                "{} {} h",
                if zh { "运行" } else { "Up" },
                m.uptime_seconds / 3_600
            )
        }),
        _ => id.strip_prefix("quotaWindow:").and_then(|window_name| {
            snapshot
                .and_then(|snapshot| {
                    snapshot
                        .windows
                        .iter()
                        .find(|window| window.name == window_name)
                })
                .map(|window| {
                    format!(
                        "{} {}% {}",
                        window.name,
                        window.remaining_percent,
                        if zh { "剩余" } else { "left" }
                    )
                })
        }),
    }
}

fn tray_view(
    preferences: &LifecyclePreferences,
    health: &SystemHealthState,
    quota: &QuotaState,
) -> TrayView {
    let snapshot = visible_snapshot(preferences, quota);
    let title = preferences
        .menu_bar
        .parameter_ids
        .iter()
        .filter_map(|id| parameter_text(id, preferences.locale, current_metrics(health), snapshot))
        .take(preferences.menu_bar.display_limit.into())
        .collect::<Vec<_>>()
        .join(" · ");
    let zh = preferences.locale == Locale::ZhCn;
    if preferences.menu_bar.pinned_account_id.is_some() && snapshot.is_none() {
        return TrayView {
            title: if title.is_empty() {
                if zh { "Codex 用量" } else { "Codex usage" }.into()
            } else {
                title
            },
            status: if zh {
                "状态：置顶账户不可用"
            } else {
                "Status: pinned account unavailable"
            }
            .into(),
            updated: if zh {
                "更新时间：暂无"
            } else {
                "Updated: unavailable"
            }
            .into(),
        };
    }
    let (status, timestamp): (&str, Option<DateTime<Utc>>) = if preferences.monitoring_paused {
        (
            if zh {
                "状态：已暂停"
            } else {
                "Status: paused"
            },
            snapshot.map(|s| s.updated_at),
        )
    } else {
        match quota {
            QuotaState::Ready { snapshot, .. } => (
                if zh {
                    "状态：额度最新"
                } else {
                    "Status: quota fresh"
                },
                Some(snapshot.updated_at),
            ),
            QuotaState::Stale { snapshot, .. } => (
                if zh {
                    "状态：额度已过期（显示可信快照）"
                } else {
                    "Status: quota stale (trusted snapshot)"
                },
                Some(snapshot.updated_at),
            ),
            QuotaState::Cooldown { snapshot, retry_at } => {
                let status = if zh {
                    format!(
                        "状态：刷新冷却中，{} UTC 后可刷新",
                        retry_at.format("%H:%M:%S")
                    )
                } else {
                    format!(
                        "Status: refresh cooldown until {} UTC",
                        retry_at.format("%H:%M:%S")
                    )
                };
                return TrayView {
                    title: if title.is_empty() {
                        if zh { "Codex 用量" } else { "Codex usage" }.into()
                    } else {
                        title
                    },
                    status,
                    updated: snapshot
                        .as_ref()
                        .map(|snapshot| {
                            format!(
                                "{} {}",
                                if zh { "更新时间：" } else { "Updated:" },
                                snapshot.updated_at.format("%Y-%m-%d %H:%M UTC")
                            )
                        })
                        .unwrap_or_else(|| {
                            if zh {
                                "更新时间：暂无"
                            } else {
                                "Updated: unavailable"
                            }
                            .into()
                        }),
                };
            }
            QuotaState::Error { last_snapshot, .. } => (
                if zh {
                    "状态：额度错误"
                } else {
                    "Status: quota error"
                },
                last_snapshot.as_ref().map(|s| s.updated_at),
            ),
            QuotaState::Loading => (
                if zh {
                    "状态：正在读取额度"
                } else {
                    "Status: loading quota"
                },
                None,
            ),
        }
    };
    TrayView {
        title: if title.is_empty() {
            if zh { "Codex 用量" } else { "Codex usage" }.into()
        } else {
            title
        },
        status: status.into(),
        updated: timestamp
            .map(|time| {
                format!(
                    "{} {}",
                    if zh { "更新时间：" } else { "Updated:" },
                    time.format("%Y-%m-%d %H:%M UTC")
                )
            })
            .unwrap_or_else(|| {
                if zh {
                    "更新时间：暂无"
                } else {
                    "Updated: unavailable"
                }
                .into()
            }),
    }
}

pub(crate) fn update_tray(app: &AppHandle, items: &TrayMenuItems) {
    let state = app.state::<AppState>();
    let preferences = state.lifecycle.preferences();
    let view = tray_view(&preferences, &state.health.latest(), &state.quota.latest());
    let (refresh, open, pause, quit) = actions(preferences.locale, preferences.monitoring_paused);
    let _ = items.status.set_text(&view.status);
    let _ = items.updated.set_text(&view.updated);
    let _ = items.refresh.set_text(refresh);
    let pinned_available = preferences.menu_bar.pinned_account_id.is_none()
        || visible_snapshot(&preferences, &state.quota.latest()).is_some();
    let _ = items
        .refresh
        .set_enabled(!preferences.monitoring_paused && pinned_available);
    let _ = items.open.set_text(open);
    let _ = items.pause.set_text(pause);
    let _ = items.quit.set_text(quit);
    if let Some(tray_icon) = app.tray_by_id("main-tray") {
        let _ = tray_icon.set_title(Some(&view.title));
        let tooltip = if preferences.locale == Locale::ZhCn {
            "Codex 用量监控"
        } else {
            "Codex Usage Monitor"
        };
        let _ = tray_icon.set_tooltip(Some(tooltip));
    }
}

pub(crate) fn setup_tray(
    app: &AppHandle,
    preferences: &LifecyclePreferences,
) -> tauri::Result<TrayMenuItems> {
    let view = tray_view(
        preferences,
        &SystemHealthState::Loading,
        &QuotaState::Loading,
    );
    let (refresh_text, open_text, pause_text, quit_text) =
        actions(preferences.locale, preferences.monitoring_paused);
    let status = MenuItem::with_id(app, "summary-status", view.status, false, None::<&str>)?;
    let updated = MenuItem::with_id(app, "summary-updated", view.updated, false, None::<&str>)?;
    let refresh = MenuItem::with_id(
        app,
        "refresh-quota",
        refresh_text,
        !preferences.monitoring_paused && preferences.menu_bar.pinned_account_id.is_none(),
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
                    && (preferences.menu_bar.pinned_account_id.is_none()
                        || visible_snapshot(&preferences, &state.quota.latest()).is_some())
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        lifecycle::MenuBarPreferences,
        quota::{QuotaAccount, QuotaWindow},
    };
    use chrono::TimeZone;

    fn snapshot(account_id: &str) -> QuotaSnapshot {
        QuotaSnapshot {
            account: QuotaAccount {
                id: account_id.into(),
                display_name: "User".into(),
                plan_type: "plus".into(),
            },
            windows: vec![QuotaWindow {
                name: "5 hours".into(),
                remaining_percent: 72,
                resets_at: None,
                window_duration_minutes: Some(300),
            }],
            updated_at: Utc.with_ymd_and_hms(2026, 7, 27, 2, 3, 0).unwrap(),
        }
    }

    fn health() -> SystemHealthState {
        SystemHealthState::Ready {
            updated_at: Utc::now(),
            metrics: SystemHealthMetrics {
                cpu_percent: 12.0,
                memory_used_bytes: 1,
                memory_total_bytes: 2,
                memory_pressure: "warning".into(),
                disk_available_bytes: 42_000_000_000,
                disk_total_bytes: 100_000_000_000,
                network_down_bytes_per_second: 1_500_000.0,
                network_up_bytes_per_second: 0.0,
                battery_percent: Some(80.0),
                battery_charging: Some(false),
                uptime_seconds: 7_200,
            },
        }
    }

    #[test]
    fn menu_bar_uses_saved_order_and_limit_with_explicit_units_and_text_status() {
        let preferences = LifecyclePreferences {
            locale: Locale::En,
            menu_bar: MenuBarPreferences {
                parameter_ids: vec![
                    "quotaWindow:5 hours".into(),
                    "networkDown".into(),
                    "cpu".into(),
                ],
                display_limit: 2,
                pinned_account_id: Some("account-1".into()),
            },
            ..LifecyclePreferences::default()
        };
        let quota = QuotaState::Ready {
            snapshot: snapshot("account-1"),
            next_refresh_at: Utc::now(),
        };

        let view = tray_view(&preferences, &health(), &quota);

        assert_eq!(view.title, "5 hours 72% left · ↓ 1.5 MB/s");
        assert_eq!(view.status, "Status: quota fresh");
        assert!(view.updated.contains("Updated: 2026-07-27 02:03 UTC"));
    }

    #[test]
    fn a_different_current_account_never_supplies_the_pinned_quota_parameter() {
        let preferences = LifecyclePreferences {
            menu_bar: MenuBarPreferences {
                parameter_ids: vec!["quotaWindow:5 hours".into(), "cpu".into()],
                display_limit: 2,
                pinned_account_id: Some("pinned-account".into()),
            },
            ..LifecyclePreferences::default()
        };
        let quota = QuotaState::Ready {
            snapshot: snapshot("current-account"),
            next_refresh_at: Utc::now(),
        };

        let view = tray_view(&preferences, &health(), &quota);
        assert_eq!(view.title, "CPU 12%");
        assert_eq!(view.status, "状态：置顶账户不可用");
    }

    #[test]
    fn lightweight_panel_names_the_manual_refresh_cooldown_without_color() {
        let preferences = LifecyclePreferences {
            locale: Locale::En,
            ..LifecyclePreferences::default()
        };
        let quota = QuotaState::Cooldown {
            snapshot: Some(snapshot("account-1")),
            retry_at: Utc.with_ymd_and_hms(2026, 7, 27, 2, 4, 30).unwrap(),
        };

        let view = tray_view(&preferences, &health(), &quota);

        assert_eq!(view.status, "Status: refresh cooldown until 02:04:30 UTC");
        assert!(view.updated.starts_with("Updated:"));
    }
}
