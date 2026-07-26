use std::sync::{
    Arc, RwLock,
    atomic::{AtomicBool, Ordering},
};

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

use crate::{
    notification::NotificationPolicy,
    quota::AccountId,
    system_health::{SystemHealthService, SystemHealthState},
};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LifecyclePreferences {
    pub monitoring_paused: bool,
    pub locale: Locale,
    pub theme: Theme,
    pub show_in_dock: bool,
    pub launch_at_login: bool,
    #[serde(default)]
    pub menu_bar: MenuBarPreferences,
    #[serde(default)]
    pub notifications: NotificationPolicy,
}

pub const MAX_MENU_BAR_PARAMETERS: u8 = 5;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MenuBarPreferences {
    pub parameter_ids: Vec<MenuBarParameter>,
    pub display_limit: u8,
    pub pinned_account_id: Option<AccountId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum MenuBarParameter {
    Cpu,
    MemoryPressure,
    DiskAvailable,
    NetworkDown,
    Battery,
    Uptime,
    QuotaWindow(String),
}

impl MenuBarParameter {
    pub fn as_id(&self) -> std::borrow::Cow<'_, str> {
        match self {
            Self::Cpu => "cpu".into(),
            Self::MemoryPressure => "memoryPressure".into(),
            Self::DiskAvailable => "diskAvailable".into(),
            Self::NetworkDown => "networkDown".into(),
            Self::Battery => "battery".into(),
            Self::Uptime => "uptime".into(),
            Self::QuotaWindow(name) => format!("quotaWindow:{name}").into(),
        }
    }
}

impl TryFrom<String> for MenuBarParameter {
    type Error = String;

    fn try_from(id: String) -> Result<Self, Self::Error> {
        match id.as_str() {
            "cpu" => Ok(Self::Cpu),
            "memoryPressure" => Ok(Self::MemoryPressure),
            "diskAvailable" => Ok(Self::DiskAvailable),
            "networkDown" => Ok(Self::NetworkDown),
            "battery" => Ok(Self::Battery),
            "uptime" => Ok(Self::Uptime),
            _ => id
                .strip_prefix("quotaWindow:")
                .filter(|name| !name.trim().is_empty() && name.len() <= 116)
                .map(|name| Self::QuotaWindow(name.to_string()))
                .ok_or_else(|| format!("unsupported menu bar parameter: {id}")),
        }
    }
}

impl Serialize for MenuBarParameter {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.as_id())
    }
}

impl<'de> Deserialize<'de> for MenuBarParameter {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::try_from(String::deserialize(deserializer)?).map_err(de::Error::custom)
    }
}

impl Default for MenuBarPreferences {
    fn default() -> Self {
        Self {
            parameter_ids: vec![
                MenuBarParameter::Cpu,
                MenuBarParameter::MemoryPressure,
                MenuBarParameter::DiskAvailable,
            ],
            display_limit: 3,
            pinned_account_id: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum Locale {
    #[serde(rename = "zh-CN")]
    ZhCn,
    #[serde(rename = "en")]
    En,
}

impl TryFrom<&str> for Locale {
    type Error = String;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "zh-CN" => Ok(Self::ZhCn),
            "en" => Ok(Self::En),
            _ => Err("unsupported locale".to_string()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    System,
    Light,
    Dark,
}

impl TryFrom<&str> for Theme {
    type Error = String;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "system" => Ok(Self::System),
            "light" => Ok(Self::Light),
            "dark" => Ok(Self::Dark),
            _ => Err("unsupported theme".to_string()),
        }
    }
}

impl Default for LifecyclePreferences {
    fn default() -> Self {
        Self {
            monitoring_paused: false,
            locale: Locale::ZhCn,
            theme: Theme::System,
            show_in_dock: false,
            launch_at_login: false,
            menu_bar: MenuBarPreferences::default(),
            notifications: NotificationPolicy::default(),
        }
    }
}

pub trait PreferenceStore: Send + Sync {
    fn load(&self) -> Result<Option<LifecyclePreferences>, String>;
    fn save(&self, preferences: &LifecyclePreferences) -> Result<(), String>;
}

pub struct LifecycleService {
    store: Arc<dyn PreferenceStore>,
    preferences: RwLock<LifecyclePreferences>,
    sampling_was_paused: AtomicBool,
}

impl LifecycleService {
    pub fn new(store: Arc<dyn PreferenceStore>) -> Result<Self, String> {
        let preferences = store.load()?.unwrap_or_default();
        let sampling_was_paused = preferences.monitoring_paused;
        Ok(Self {
            store,
            preferences: RwLock::new(preferences),
            sampling_was_paused: AtomicBool::new(sampling_was_paused),
        })
    }

    pub fn preferences(&self) -> LifecyclePreferences {
        self.preferences
            .read()
            .expect("preferences poisoned")
            .clone()
    }

    pub fn set_monitoring_paused(&self, paused: bool) -> Result<LifecyclePreferences, String> {
        let preferences = self.update(|preferences| preferences.monitoring_paused = paused)?;
        if paused {
            self.sampling_was_paused.store(true, Ordering::Release);
        }
        Ok(preferences)
    }

    pub fn set_theme(&self, theme: &str) -> Result<LifecyclePreferences, String> {
        let theme = Theme::try_from(theme)?;
        self.update(|preferences| preferences.theme = theme)
    }

    pub fn set_locale(&self, locale: &str) -> Result<LifecyclePreferences, String> {
        let locale = Locale::try_from(locale)?;
        self.update(|preferences| preferences.locale = locale)
    }

    pub fn set_menu_bar(
        &self,
        menu_bar: MenuBarPreferences,
    ) -> Result<LifecyclePreferences, String> {
        validate_menu_bar(&menu_bar)?;
        self.update(|preferences| preferences.menu_bar = menu_bar)
    }

    pub fn set_notifications(
        &self,
        notifications: NotificationPolicy,
    ) -> Result<LifecyclePreferences, String> {
        notifications.validate()?;
        self.update(|preferences| preferences.notifications = notifications)
    }

    fn update(
        &self,
        change: impl FnOnce(&mut LifecyclePreferences),
    ) -> Result<LifecyclePreferences, String> {
        let mut current = self.preferences.write().expect("preferences poisoned");
        let mut next = current.clone();
        change(&mut next);
        self.store.save(&next)?;
        *current = next.clone();
        Ok(next)
    }

    pub fn sample_if_active(
        &self,
        health: &SystemHealthService,
    ) -> Result<Option<SystemHealthState>, String> {
        if self.preferences().monitoring_paused {
            Ok(None)
        } else {
            if self.sampling_was_paused.swap(false, Ordering::AcqRel) {
                health.reset_rate_baseline();
            }
            health.sample().map(Some)
        }
    }
}

fn validate_menu_bar(menu_bar: &MenuBarPreferences) -> Result<(), String> {
    if !(1..=MAX_MENU_BAR_PARAMETERS).contains(&menu_bar.display_limit) {
        return Err(format!(
            "display limit must be between 1 and {MAX_MENU_BAR_PARAMETERS}"
        ));
    }
    if menu_bar.parameter_ids.len() > 12 {
        return Err("too many menu bar parameters".to_string());
    }
    let mut unique = std::collections::HashSet::new();
    for parameter in &menu_bar.parameter_ids {
        if !unique.insert(parameter) {
            return Err(format!(
                "duplicate menu bar parameter: {}",
                parameter.as_id()
            ));
        }
    }
    if menu_bar
        .pinned_account_id
        .as_ref()
        .is_some_and(|id| id.as_str().trim().is_empty() || id.as_str().len() > 256)
    {
        return Err("pinned account id must not be empty".to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    };

    use chrono::{TimeZone, Utc};

    use crate::system_health::{MemoryPressureLevel, MetricSnapshot, MetricSource};

    use super::*;

    #[derive(Default)]
    struct MemoryPreferenceStore {
        value: Mutex<Option<LifecyclePreferences>>,
    }

    impl PreferenceStore for MemoryPreferenceStore {
        fn load(&self) -> Result<Option<LifecyclePreferences>, String> {
            Ok(self.value.lock().unwrap().clone())
        }

        fn save(&self, preferences: &LifecyclePreferences) -> Result<(), String> {
            *self.value.lock().unwrap() = Some(preferences.clone());
            Ok(())
        }
    }

    struct CountingSource(AtomicUsize);

    impl MetricSource for CountingSource {
        fn collect(&self) -> Result<MetricSnapshot, String> {
            let sample_index = self.0.fetch_add(1, Ordering::SeqCst);
            Ok(MetricSnapshot {
                observed_at: Utc
                    .with_ymd_and_hms(2026, 7, 26, 10, 0, sample_index as u32 * 2)
                    .unwrap(),
                cpu_percent: 1.0,
                memory_used_bytes: 1,
                memory_total_bytes: 2,
                memory_pressure: MemoryPressureLevel::Normal,
                disk_available_bytes: 1,
                disk_total_bytes: 2,
                network_received_bytes: sample_index as u64 * 2_000,
                network_transmitted_bytes: sample_index as u64 * 1_000,
                battery_percent: None,
                battery_charging: None,
                uptime_seconds: 1,
            })
        }
    }

    #[test]
    fn pausing_stops_sampling_and_survives_service_restart() {
        let store = Arc::new(MemoryPreferenceStore::default());
        let lifecycle = LifecycleService::new(store.clone()).unwrap();
        let source = Arc::new(CountingSource(AtomicUsize::new(0)));
        let health = SystemHealthService::new(source.clone());

        lifecycle.sample_if_active(&health).unwrap();
        lifecycle.set_monitoring_paused(true).unwrap();
        assert_eq!(lifecycle.sample_if_active(&health).unwrap(), None);
        lifecycle.set_monitoring_paused(false).unwrap();
        let resumed = lifecycle.sample_if_active(&health).unwrap().unwrap();
        let SystemHealthState::Ready { metrics, .. } = resumed else {
            panic!("expected ready state after resume");
        };
        assert_eq!(metrics.network_down_bytes_per_second, 0.0);
        assert_eq!(metrics.network_up_bytes_per_second, 0.0);
        assert_eq!(source.0.load(Ordering::SeqCst), 2);

        lifecycle.set_monitoring_paused(true).unwrap();

        let restarted = LifecycleService::new(store).unwrap();
        assert!(restarted.preferences().monitoring_paused);
    }

    #[test]
    fn rejects_unknown_locales_and_themes_at_the_service_boundary() {
        let lifecycle = LifecycleService::new(Arc::new(MemoryPreferenceStore::default())).unwrap();

        assert_eq!(
            lifecycle.set_locale("fr"),
            Err("unsupported locale".to_string())
        );
        assert_eq!(
            lifecycle.set_theme("sepia"),
            Err("unsupported theme".to_string())
        );
        assert_eq!(lifecycle.preferences(), LifecyclePreferences::default());
    }

    #[test]
    fn saves_ordered_menu_bar_parameters_and_rejects_invalid_or_duplicate_ids() {
        let store = Arc::new(MemoryPreferenceStore::default());
        let lifecycle = LifecycleService::new(store.clone()).unwrap();
        let saved = lifecycle
            .set_menu_bar(MenuBarPreferences {
                parameter_ids: vec![
                    "quotaWindow:codex primary".to_string().try_into().unwrap(),
                    MenuBarParameter::Cpu,
                ],
                display_limit: 2,
                pinned_account_id: Some("account-42".into()),
            })
            .unwrap();

        assert_eq!(
            saved.menu_bar.parameter_ids[0].as_id(),
            "quotaWindow:codex primary"
        );
        assert_eq!(
            saved
                .menu_bar
                .pinned_account_id
                .as_ref()
                .map(AccountId::as_str),
            Some("account-42")
        );
        assert_eq!(LifecycleService::new(store).unwrap().preferences(), saved);

        assert!(
            lifecycle
                .set_menu_bar(MenuBarPreferences {
                    parameter_ids: vec![MenuBarParameter::Cpu, MenuBarParameter::Cpu],
                    display_limit: 2,
                    pinned_account_id: None,
                })
                .unwrap_err()
                .contains("duplicate")
        );
        assert!(
            lifecycle
                .set_menu_bar(MenuBarPreferences {
                    parameter_ids: vec![MenuBarParameter::Cpu],
                    display_limit: MAX_MENU_BAR_PARAMETERS + 1,
                    pinned_account_id: None,
                })
                .is_err()
        );
        assert!(serde_json::from_str::<MenuBarParameter>(r#""secretTokens""#).is_err());
    }

    #[test]
    fn notification_defaults_are_actionable_and_invalid_thresholds_are_rejected() {
        let lifecycle = LifecycleService::new(Arc::new(MemoryPreferenceStore::default())).unwrap();

        assert_eq!(
            lifecycle.preferences().notifications.quota_thresholds,
            vec![20, 10, 0]
        );
        let mut invalid = lifecycle.preferences().notifications;
        invalid.quota_thresholds = vec![20, 20];

        assert_eq!(
            lifecycle.set_notifications(invalid),
            Err("quota thresholds must be unique".to_string())
        );
    }
}
