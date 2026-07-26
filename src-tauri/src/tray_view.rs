use chrono::{DateTime, Local, Utc};

use crate::{
    lifecycle::{LifecyclePreferences, Locale, MenuBarParameter},
    quota::{QuotaSnapshot, QuotaState},
    system_health::{SystemHealthMetrics, SystemHealthState},
};

#[derive(Debug, PartialEq)]
pub(crate) struct TrayView {
    pub(crate) title: String,
    pub(crate) status: String,
    pub(crate) updated: String,
}

#[derive(Clone, Copy)]
enum ParameterSource {
    System,
    Quota,
}

pub(crate) fn pinned_quota_available(
    preferences: &LifecyclePreferences,
    quota: &QuotaState,
) -> bool {
    preferences.menu_bar.pinned_account_id.is_none()
        || visible_snapshot(preferences, quota).is_some()
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

fn decimal_separator(os_locale: &str) -> char {
    let language = os_locale.split(['-', '_']).next().unwrap_or("en");
    if matches!(
        language,
        "de" | "fr" | "es" | "it" | "pt" | "ru" | "nl" | "pl" | "tr" | "id" | "vi"
    ) {
        ','
    } else {
        '.'
    }
}

fn format_number(value: f64, decimals: usize, os_locale: &str) -> String {
    let formatted = format!("{value:.decimals$}");
    if decimal_separator(os_locale) == ',' {
        formatted.replace('.', ",")
    } else {
        formatted
    }
}

fn format_datetime(time: DateTime<Utc>, os_locale: &str) -> String {
    let local = time.with_timezone(&Local);
    if os_locale.eq_ignore_ascii_case("en-US") || os_locale.starts_with("en_US") {
        local.format("%m/%d/%Y %I:%M %p").to_string()
    } else if os_locale.starts_with("zh") {
        local.format("%Y/%m/%d %H:%M").to_string()
    } else {
        local.format("%d/%m/%Y %H:%M").to_string()
    }
}

fn format_time(time: DateTime<Utc>, os_locale: &str) -> String {
    let local = time.with_timezone(&Local);
    if os_locale.eq_ignore_ascii_case("en-US") || os_locale.starts_with("en_US") {
        local.format("%I:%M:%S %p").to_string()
    } else {
        local.format("%H:%M:%S").to_string()
    }
}

fn parameter_text(
    parameter: &MenuBarParameter,
    locale: Locale,
    os_locale: &str,
    metrics: Option<&SystemHealthMetrics>,
    snapshot: Option<&QuotaSnapshot>,
) -> Option<(String, ParameterSource)> {
    let zh = locale == Locale::ZhCn;
    let system = |text| Some((text, ParameterSource::System));
    match parameter {
        MenuBarParameter::Cpu => metrics.and_then(|m| {
            system(format!(
                "CPU {}%",
                format_number(f64::from(m.cpu_percent), 0, os_locale)
            ))
        }),
        MenuBarParameter::MemoryPressure => metrics.and_then(|m| {
            let state = match (zh, m.memory_pressure.as_str()) {
                (true, "normal") => "正常",
                (true, "warning") => "偏高",
                (true, _) => "严重",
                (false, "normal") => "normal",
                (false, "warning") => "high",
                (false, _) => "critical",
            };
            system(format!("{} {state}", if zh { "内存" } else { "Memory" }))
        }),
        MenuBarParameter::DiskAvailable => metrics.and_then(|m| {
            system(format!(
                "{} {} GB",
                if zh { "磁盘" } else { "Disk" },
                format_number(
                    m.disk_available_bytes as f64 / 1_000_000_000.0,
                    0,
                    os_locale
                )
            ))
        }),
        MenuBarParameter::NetworkDown => metrics.and_then(|m| {
            system(format!(
                "↓ {} MB/s",
                format_number(m.network_down_bytes_per_second / 1_000_000.0, 1, os_locale)
            ))
        }),
        MenuBarParameter::Battery => metrics.and_then(|m| m.battery_percent).and_then(|value| {
            system(format!(
                "{} {}%",
                if zh { "电量" } else { "Battery" },
                format_number(f64::from(value), 0, os_locale)
            ))
        }),
        MenuBarParameter::Uptime => metrics.and_then(|m| {
            system(format!(
                "{} {} h",
                if zh { "运行" } else { "Up" },
                m.uptime_seconds / 3_600
            ))
        }),
        MenuBarParameter::QuotaWindow(window_name) => snapshot.and_then(|snapshot| {
            snapshot
                .windows
                .iter()
                .find(|window| window.name == *window_name)
                .map(|window| {
                    (
                        format!(
                            "{} {}% {}",
                            window.name,
                            window.remaining_percent,
                            if zh { "剩余" } else { "left" }
                        ),
                        ParameterSource::Quota,
                    )
                })
        }),
    }
}

fn system_status(
    locale: Locale,
    paused: bool,
    health: &SystemHealthState,
) -> (&'static str, Option<DateTime<Utc>>) {
    let zh = locale == Locale::ZhCn;
    if paused {
        return (
            if zh {
                "系统已暂停"
            } else {
                "system paused"
            },
            health_time(health),
        );
    }
    match health {
        SystemHealthState::Loading => (
            if zh {
                "系统读取中"
            } else {
                "system loading"
            },
            None,
        ),
        SystemHealthState::Ready { updated_at, .. } => (
            if zh { "系统正常" } else { "system fresh" },
            Some(*updated_at),
        ),
        SystemHealthState::Stale { updated_at, .. } => (
            if zh {
                "系统已过期"
            } else {
                "system stale"
            },
            Some(*updated_at),
        ),
        SystemHealthState::Error { updated_at, .. } => (
            if zh { "系统错误" } else { "system error" },
            Some(*updated_at),
        ),
    }
}

fn health_time(health: &SystemHealthState) -> Option<DateTime<Utc>> {
    match health {
        SystemHealthState::Ready { updated_at, .. }
        | SystemHealthState::Stale { updated_at, .. }
        | SystemHealthState::Error { updated_at, .. } => Some(*updated_at),
        SystemHealthState::Loading => None,
    }
}

fn quota_status(
    preferences: &LifecyclePreferences,
    quota: &QuotaState,
    os_locale: &str,
) -> (String, Option<DateTime<Utc>>) {
    let zh = preferences.locale == Locale::ZhCn;
    if preferences.menu_bar.pinned_account_id.is_some()
        && visible_snapshot(preferences, quota).is_none()
    {
        return (
            if zh {
                "置顶账户不可用"
            } else {
                "pinned account unavailable"
            }
            .into(),
            None,
        );
    }
    if preferences.monitoring_paused {
        return (
            if zh {
                "额度已暂停"
            } else {
                "quota paused"
            }
            .into(),
            visible_snapshot(preferences, quota).map(|snapshot| snapshot.updated_at),
        );
    }
    match quota {
        QuotaState::Loading => (
            if zh {
                "额度读取中"
            } else {
                "quota loading"
            }
            .into(),
            None,
        ),
        QuotaState::Ready { snapshot, .. } => (
            if zh { "额度最新" } else { "quota fresh" }.into(),
            Some(snapshot.updated_at),
        ),
        QuotaState::Stale { snapshot, .. } => (
            if zh {
                "额度已过期（可信快照）"
            } else {
                "quota stale (trusted snapshot)"
            }
            .into(),
            Some(snapshot.updated_at),
        ),
        QuotaState::Cooldown { snapshot, retry_at } => (
            format!(
                "{} {}",
                if zh {
                    "刷新冷却至"
                } else {
                    "refresh cooldown until"
                },
                format_time(*retry_at, os_locale)
            ),
            snapshot.as_ref().map(|snapshot| snapshot.updated_at),
        ),
        QuotaState::Error { last_snapshot, .. } => (
            if zh { "额度错误" } else { "quota error" }.into(),
            last_snapshot.as_ref().map(|snapshot| snapshot.updated_at),
        ),
    }
}

fn title_or_default(title: String, zh: bool) -> String {
    if title.is_empty() {
        if zh { "Codex 用量" } else { "Codex usage" }.into()
    } else {
        title
    }
}

fn format_updated(
    system_time: Option<DateTime<Utc>>,
    quota_time: Option<DateTime<Utc>>,
    locale: Locale,
    os_locale: &str,
) -> String {
    let zh = locale == Locale::ZhCn;
    let mut parts = Vec::new();
    if let Some(time) = system_time {
        parts.push(format!(
            "{} {}",
            if zh { "系统更新" } else { "System updated" },
            format_datetime(time, os_locale)
        ));
    }
    if let Some(time) = quota_time {
        parts.push(format!(
            "{} {}",
            if zh { "额度更新" } else { "Quota updated" },
            format_datetime(time, os_locale)
        ));
    }
    if parts.is_empty() {
        if zh {
            "更新时间：暂无"
        } else {
            "Updated: unavailable"
        }
        .into()
    } else {
        parts.join(if zh { "；" } else { "; " })
    }
}

pub(crate) fn build_tray_view(
    preferences: &LifecyclePreferences,
    health: &SystemHealthState,
    quota: &QuotaState,
    os_locale: &str,
) -> TrayView {
    let snapshot = visible_snapshot(preferences, quota);
    let selected = preferences
        .menu_bar
        .parameter_ids
        .iter()
        .take(preferences.menu_bar.display_limit.into());
    let mut title_parts = Vec::new();
    let mut has_system = false;
    let mut has_quota = false;
    for parameter in selected {
        match parameter {
            MenuBarParameter::QuotaWindow(_) => has_quota = true,
            _ => has_system = true,
        }
        if let Some((text, source)) = parameter_text(
            parameter,
            preferences.locale,
            os_locale,
            current_metrics(health),
            snapshot,
        ) {
            match source {
                ParameterSource::System => has_system = true,
                ParameterSource::Quota => has_quota = true,
            }
            title_parts.push(text);
        }
    }
    let zh = preferences.locale == Locale::ZhCn;
    let mut statuses = Vec::new();
    let mut system_time = None;
    let mut quota_time = None;
    if has_system {
        let (status, time) =
            system_status(preferences.locale, preferences.monitoring_paused, health);
        statuses.push(status.to_string());
        system_time = time;
    }
    if has_quota {
        let (status, time) = quota_status(preferences, quota, os_locale);
        statuses.push(status);
        quota_time = time;
    }
    if statuses.is_empty() {
        statuses.push(
            if zh {
                "未选择参数"
            } else {
                "no parameters selected"
            }
            .into(),
        );
    }
    TrayView {
        title: title_or_default(title_parts.join(" · "), zh),
        status: format!(
            "{}{}",
            if zh { "状态：" } else { "Status: " },
            statuses.join(if zh { "；" } else { "; " })
        ),
        updated: format_updated(system_time, quota_time, preferences.locale, os_locale),
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;
    use crate::{
        lifecycle::MenuBarPreferences,
        quota::{QuotaAccount, QuotaWindow},
    };

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
            updated_at: Utc.with_ymd_and_hms(2026, 7, 27, 2, 2, 0).unwrap(),
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
    fn ordered_limit_values_units_and_quota_status_are_visible() {
        let preferences = LifecyclePreferences {
            locale: Locale::En,
            menu_bar: MenuBarPreferences {
                parameter_ids: vec![
                    MenuBarParameter::QuotaWindow("5 hours".into()),
                    MenuBarParameter::NetworkDown,
                    MenuBarParameter::Cpu,
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

        let view = build_tray_view(&preferences, &health(), &quota, "de-DE");

        assert_eq!(view.title, "5 hours 72% left · ↓ 1,5 MB/s");
        assert_eq!(view.status, "Status: system fresh; quota fresh");
        assert!(view.updated.contains("Quota updated"));
        assert!(!view.updated.contains("UTC"));
    }

    #[test]
    fn system_only_parameters_use_health_status_and_health_update_time() {
        let preferences = LifecyclePreferences::default();
        let quota = QuotaState::Error {
            reason: crate::quota::QuotaErrorReason::Transport,
            last_snapshot: None,
            failed_at: Utc::now(),
            retry_at: None,
        };

        let view = build_tray_view(&preferences, &health(), &quota, "zh-CN");

        assert_eq!(view.status, "状态：系统正常");
        assert!(view.updated.starts_with("系统更新"));
        assert!(!view.updated.contains("额度"));
    }

    #[test]
    fn another_current_account_never_supplies_pinned_quota() {
        let preferences = LifecyclePreferences {
            menu_bar: MenuBarPreferences {
                parameter_ids: vec![
                    MenuBarParameter::QuotaWindow("5 hours".into()),
                    MenuBarParameter::Cpu,
                ],
                display_limit: 2,
                pinned_account_id: Some("pinned-account".into()),
            },
            ..LifecyclePreferences::default()
        };
        let quota = QuotaState::Ready {
            snapshot: snapshot("current-account"),
            next_refresh_at: Utc::now(),
        };

        let view = build_tray_view(&preferences, &health(), &quota, "zh-CN");

        assert_eq!(view.title, "CPU 12%");
        assert_eq!(view.status, "状态：系统正常；置顶账户不可用");
        assert!(!pinned_quota_available(&preferences, &quota));
    }

    #[test]
    fn lightweight_panel_names_manual_cooldown_in_os_local_time() {
        let preferences = LifecyclePreferences {
            locale: Locale::En,
            menu_bar: MenuBarPreferences {
                parameter_ids: vec![MenuBarParameter::QuotaWindow("5 hours".into())],
                display_limit: 1,
                pinned_account_id: None,
            },
            ..LifecyclePreferences::default()
        };
        let quota = QuotaState::Cooldown {
            snapshot: Some(snapshot("account-1")),
            retry_at: Utc.with_ymd_and_hms(2026, 7, 27, 2, 4, 30).unwrap(),
        };

        let view = build_tray_view(&preferences, &health(), &quota, "en-US");

        assert!(view.status.contains("refresh cooldown until"));
        assert!(view.status.contains("AM") || view.status.contains("PM"));
        assert!(!view.status.contains("UTC"));
    }
}
