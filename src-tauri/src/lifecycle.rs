use std::sync::{Arc, RwLock};

use serde::{Deserialize, Serialize};

use crate::system_health::{SystemHealthService, SystemHealthState};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LifecyclePreferences {
    pub monitoring_paused: bool,
    pub locale: Locale,
    pub theme: Theme,
    pub show_in_dock: bool,
    pub launch_at_login: bool,
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
}

impl LifecycleService {
    pub fn new(store: Arc<dyn PreferenceStore>) -> Result<Self, String> {
        let preferences = store.load()?.unwrap_or_default();
        Ok(Self {
            store,
            preferences: RwLock::new(preferences),
        })
    }

    pub fn preferences(&self) -> LifecyclePreferences {
        self.preferences
            .read()
            .expect("preferences poisoned")
            .clone()
    }

    pub fn set_monitoring_paused(&self, paused: bool) -> Result<LifecyclePreferences, String> {
        self.update(|preferences| preferences.monitoring_paused = paused)
    }

    pub fn set_theme(&self, theme: &str) -> Result<LifecyclePreferences, String> {
        let theme = Theme::try_from(theme)?;
        self.update(|preferences| preferences.theme = theme)
    }

    pub fn set_locale(&self, locale: &str) -> Result<LifecyclePreferences, String> {
        let locale = Locale::try_from(locale)?;
        self.update(|preferences| preferences.locale = locale)
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
            health.sample().map(Some)
        }
    }
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
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(MetricSnapshot {
                observed_at: Utc.with_ymd_and_hms(2026, 7, 26, 10, 0, 0).unwrap(),
                cpu_percent: 1.0,
                memory_used_bytes: 1,
                memory_total_bytes: 2,
                memory_pressure: MemoryPressureLevel::Normal,
                disk_available_bytes: 1,
                disk_total_bytes: 2,
                network_received_bytes: 0,
                network_transmitted_bytes: 0,
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
        assert_eq!(source.0.load(Ordering::SeqCst), 1);

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
}
