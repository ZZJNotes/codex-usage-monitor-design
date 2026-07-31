use chrono::{DateTime, Local, Utc};

use crate::{
    lifecycle::{LifecyclePreferences, Locale, MenuBarParameter},
    quota::{QuotaSnapshot, QuotaState},
    system_health::{SystemHealthMetrics, SystemHealthState},
};

#[allow(dead_code)]
#[derive(Debug, PartialEq)]
pub(crate) struct TrayView {
    pub(crate) title: String,
    pub(crate) status: String,
    pub(crate) updated: String,
    pub(crate) reset: String,
}

pub(crate) struct TrayCopy {
    #[allow(dead_code)]
    pub(crate) refresh_current: &'static str,
    #[allow(dead_code)]
    pub(crate) refresh_pinned: &'static str,
    #[allow(dead_code)]
    pub(crate) refresh_generic: &'static str,
    #[allow(dead_code)]
    pub(crate) open: &'static str,
    #[allow(dead_code)]
    pub(crate) pause: &'static str,
    #[allow(dead_code)]
    pub(crate) resume: &'static str,
    #[allow(dead_code)]
    pub(crate) quit: &'static str,
    #[allow(dead_code)]
    pub(crate) tooltip: &'static str,
    default_title: &'static str,
    system_paused: &'static str,
    system_loading: &'static str,
    system_fresh: &'static str,
    system_stale: &'static str,
    system_error: &'static str,
    quota_paused: &'static str,
    quota_loading: &'static str,
    quota_fresh: &'static str,
    quota_stale: &'static str,
    quota_error: &'static str,
    pinned_unavailable: &'static str,
    cooldown_until: &'static str,
    status_prefix: &'static str,
    system_updated: &'static str,
    quota_updated: &'static str,
    updated_unavailable: &'static str,
    reset_prefix: &'static str,
    reset_unavailable: &'static str,
    no_quota_selected: &'static str,
    no_parameters: &'static str,
    separator: &'static str,
}

pub(crate) fn tray_copy(locale: Locale) -> TrayCopy {
    match locale {
        Locale::ZhCn => TrayCopy {
            refresh_current: "刷新当前账户",
            refresh_pinned: "刷新置顶账户",
            refresh_generic: "刷新额度",
            open: "打开仪表盘",
            pause: "暂停监控",
            resume: "恢复监控",
            quit: "退出",
            tooltip: "Codex 用量监控",
            default_title: "Codex 用量",
            system_paused: "系统已暂停",
            system_loading: "系统读取中",
            system_fresh: "系统正常",
            system_stale: "系统已过期",
            system_error: "系统错误",
            quota_paused: "额度已暂停",
            quota_loading: "额度读取中",
            quota_fresh: "额度最新",
            quota_stale: "额度已过期（可信快照）",
            quota_error: "额度错误",
            pinned_unavailable: "置顶账户不可用",
            cooldown_until: "刷新冷却至",
            status_prefix: "状态：",
            system_updated: "系统更新",
            quota_updated: "额度更新",
            updated_unavailable: "更新时间：暂无",
            reset_prefix: "额度重置：",
            reset_unavailable: "不可用",
            no_quota_selected: "未选择额度参数",
            no_parameters: "未选择参数",
            separator: "；",
        },
        Locale::En => TrayCopy {
            refresh_current: "Refresh current account",
            refresh_pinned: "Refresh pinned account",
            refresh_generic: "Refresh quota",
            open: "Open dashboard",
            pause: "Pause monitoring",
            resume: "Resume monitoring",
            quit: "Quit",
            tooltip: "Codex Usage Monitor",
            default_title: "Codex usage",
            system_paused: "system paused",
            system_loading: "system loading",
            system_fresh: "system fresh",
            system_stale: "system stale",
            system_error: "system error",
            quota_paused: "quota paused",
            quota_loading: "quota loading",
            quota_fresh: "quota fresh",
            quota_stale: "quota stale (trusted snapshot)",
            quota_error: "quota error",
            pinned_unavailable: "pinned account unavailable",
            cooldown_until: "refresh cooldown until",
            status_prefix: "Status: ",
            system_updated: "System updated",
            quota_updated: "Quota updated",
            updated_unavailable: "Updated: unavailable",
            reset_prefix: "Quota reset: ",
            reset_unavailable: "unavailable",
            no_quota_selected: "no quota parameter selected",
            no_parameters: "no parameters selected",
            separator: "; ",
        },
    }
}

#[allow(dead_code)]
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
    let snapshot = quota.snapshot()?;
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

fn parameter_compact(
    parameter: &MenuBarParameter,
    os_locale: &str,
    metrics: Option<&SystemHealthMetrics>,
    snapshot: Option<&QuotaSnapshot>,
) -> Option<String> {
    match parameter {
        MenuBarParameter::Cpu => metrics.map(|m| {
            format!(
                "C{}%",
                format_number(f64::from(m.cpu_percent), 0, os_locale)
            )
        }),
        MenuBarParameter::MemoryPressure => metrics.map(|m| {
            let percent = if m.memory_total_bytes == 0 {
                0.0
            } else {
                m.memory_used_bytes as f64 / m.memory_total_bytes as f64 * 100.0
            };
            format!("M{}%", format_number(percent, 0, os_locale))
        }),
        MenuBarParameter::DiskAvailable => metrics.map(|m| {
            let percent = if m.disk_total_bytes == 0 {
                0.0
            } else {
                m.disk_available_bytes as f64 / m.disk_total_bytes as f64 * 100.0
            };
            format!("D{}%", format_number(percent, 0, os_locale))
        }),
        MenuBarParameter::NetworkDown => metrics.map(|m| {
            format!(
                "N{}M",
                format_number(m.network_down_bytes_per_second / 1_000_000.0, 1, os_locale)
            )
        }),
        MenuBarParameter::Battery => metrics
            .and_then(|m| m.battery_percent)
            .map(|value| format!("B{}%", format_number(f64::from(value), 0, os_locale))),
        MenuBarParameter::Uptime => metrics.map(|m| format!("U{}H", m.uptime_seconds / 3_600)),
        MenuBarParameter::QuotaWindow(name) => snapshot
            .and_then(|snapshot| snapshot.windows.iter().find(|window| window.name == *name))
            .map(|window| format!("Q{}%", window.remaining_percent)),
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

fn system_status<'a>(
    copy: &'a TrayCopy,
    paused: bool,
    health: &SystemHealthState,
) -> (&'a str, Option<DateTime<Utc>>) {
    if paused {
        return (copy.system_paused, health_time(health));
    }
    match health {
        SystemHealthState::Loading => (copy.system_loading, None),
        SystemHealthState::Ready { updated_at, .. } => (copy.system_fresh, Some(*updated_at)),
        SystemHealthState::Stale { updated_at, .. } => (copy.system_stale, Some(*updated_at)),
        SystemHealthState::Error { updated_at, .. } => (copy.system_error, Some(*updated_at)),
    }
}

fn quota_status(
    preferences: &LifecyclePreferences,
    quota: &QuotaState,
    copy: &TrayCopy,
    os_locale: &str,
) -> (String, Option<DateTime<Utc>>) {
    if preferences.menu_bar.pinned_account_id.is_some()
        && visible_snapshot(preferences, quota).is_none()
    {
        return (copy.pinned_unavailable.into(), None);
    }
    if preferences.monitoring_paused {
        return (
            copy.quota_paused.into(),
            visible_snapshot(preferences, quota).map(|snapshot| snapshot.updated_at),
        );
    }
    match quota {
        QuotaState::Loading => (copy.quota_loading.into(), None),
        QuotaState::Ready { snapshot, .. } => (copy.quota_fresh.into(), Some(snapshot.updated_at)),
        QuotaState::Stale { snapshot, .. } => (copy.quota_stale.into(), Some(snapshot.updated_at)),
        QuotaState::Cooldown { snapshot, retry_at } => (
            format!(
                "{} {}",
                copy.cooldown_until,
                format_time(*retry_at, os_locale)
            ),
            snapshot.as_ref().map(|snapshot| snapshot.updated_at),
        ),
        QuotaState::Error { last_snapshot, .. } => (
            copy.quota_error.into(),
            last_snapshot.as_ref().map(|snapshot| snapshot.updated_at),
        ),
    }
}

fn format_updated(
    system_time: Option<DateTime<Utc>>,
    quota_time: Option<DateTime<Utc>>,
    copy: &TrayCopy,
    os_locale: &str,
) -> String {
    let mut parts = Vec::new();
    if let Some(time) = system_time {
        parts.push(format!(
            "{} {}",
            copy.system_updated,
            format_datetime(time, os_locale)
        ));
    }
    if let Some(time) = quota_time {
        parts.push(format!(
            "{} {}",
            copy.quota_updated,
            format_datetime(time, os_locale)
        ));
    }
    if parts.is_empty() {
        copy.updated_unavailable.into()
    } else {
        parts.join(copy.separator)
    }
}

fn format_resets(
    selected: &[&MenuBarParameter],
    snapshot: Option<&QuotaSnapshot>,
    copy: &TrayCopy,
    os_locale: &str,
) -> String {
    let quota_names = selected
        .iter()
        .filter_map(|parameter| match parameter {
            MenuBarParameter::QuotaWindow(name) => Some(name),
            _ => None,
        })
        .collect::<Vec<_>>();
    if quota_names.is_empty() {
        return format!("{}{}", copy.reset_prefix, copy.no_quota_selected);
    }
    let resets = quota_names
        .into_iter()
        .map(|name| {
            let value = snapshot
                .and_then(|snapshot| snapshot.windows.iter().find(|window| window.name == *name))
                .and_then(|window| window.resets_at)
                .map(|time| format_datetime(time, os_locale))
                .unwrap_or_else(|| copy.reset_unavailable.into());
            format!("{name} {value}")
        })
        .collect::<Vec<_>>()
        .join(copy.separator);
    format!("{}{resets}", copy.reset_prefix)
}

pub(crate) fn build_tray_view(
    preferences: &LifecyclePreferences,
    health: &SystemHealthState,
    quota: &QuotaState,
    os_locale: &str,
) -> TrayView {
    let copy = tray_copy(preferences.locale);
    let snapshot = visible_snapshot(preferences, quota);
    let selected = preferences
        .menu_bar
        .parameter_ids
        .iter()
        .collect::<Vec<_>>();
    let has_system = selected
        .iter()
        .any(|parameter| !matches!(parameter, MenuBarParameter::QuotaWindow(_)));
    let has_quota = selected
        .iter()
        .any(|parameter| matches!(parameter, MenuBarParameter::QuotaWindow(_)));
    let title = selected
        .iter()
        .filter_map(|parameter| {
            parameter_compact(parameter, os_locale, current_metrics(health), snapshot)
        })
        .take(preferences.menu_bar.display_limit.into())
        .collect::<Vec<_>>()
        .join(" ");
    let mut statuses = Vec::new();
    let mut system_time = None;
    let mut quota_time = None;
    if has_system {
        let (status, time) = system_status(&copy, preferences.monitoring_paused, health);
        statuses.push(status.to_string());
        system_time = time;
    }
    if has_quota {
        let (status, time) = quota_status(preferences, quota, &copy, os_locale);
        statuses.push(status);
        quota_time = time;
    }
    if statuses.is_empty() {
        statuses.push(copy.no_parameters.into());
    }
    TrayView {
        title: if title.is_empty() {
            copy.default_title.into()
        } else {
            title
        },
        status: format!("{}{}", copy.status_prefix, statuses.join(copy.separator)),
        updated: format_updated(system_time, quota_time, &copy, os_locale),
        reset: format_resets(&selected, snapshot, &copy, os_locale),
    }
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
                resets_at: Some(Utc.with_ymd_and_hms(2026, 7, 27, 3, 0, 0).unwrap()),
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
    fn ordered_values_reset_time_and_os_locale_are_visible() {
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
        assert_eq!(view.title, "Q72% N1,5M");
        assert_eq!(view.status, "Status: system fresh; quota fresh");
        assert!(view.reset.starts_with("Quota reset: 5 hours "));
        assert!(!view.reset.contains("UTC"));
    }

    #[test]
    fn system_only_parameters_use_health_state_and_time() {
        let quota = QuotaState::Error {
            reason: crate::quota::QuotaErrorReason::Transport,
            last_snapshot: None,
            failed_at: Utc::now(),
            retry_at: None,
        };
        let view = build_tray_view(&LifecyclePreferences::default(), &health(), &quota, "zh-CN");
        assert_eq!(view.title, "C12% M50% D42%");
        assert_eq!(view.status, "状态：系统正常");
        assert!(view.updated.starts_with("系统更新"));
        assert!(!view.updated.contains("额度"));
    }

    #[test]
    fn another_account_never_supplies_pinned_quota_or_refresh() {
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
        assert_eq!(view.title, "C12%");
        assert_eq!(view.status, "状态：系统正常；置顶账户不可用");
        assert!(!pinned_quota_available(&preferences, &quota));
    }

    #[test]
    fn panel_names_manual_cooldown_in_os_local_time() {
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

    #[test]
    fn an_unavailable_window_does_not_consume_the_visible_parameter_limit() {
        let preferences = LifecyclePreferences {
            locale: Locale::En,
            menu_bar: MenuBarPreferences {
                parameter_ids: vec![
                    MenuBarParameter::QuotaWindow("retired".into()),
                    MenuBarParameter::Cpu,
                    MenuBarParameter::NetworkDown,
                ],
                display_limit: 2,
                pinned_account_id: None,
            },
            ..LifecyclePreferences::default()
        };
        let quota = QuotaState::Ready {
            snapshot: snapshot("account-1"),
            next_refresh_at: Utc::now(),
        };

        let view = build_tray_view(&preferences, &health(), &quota, "en-US");

        assert_eq!(view.title, "C12% N1.5M");
        assert!(view.reset.contains("retired unavailable"));
    }
}
