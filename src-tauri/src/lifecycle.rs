use std::sync::{
    Arc, RwLock,
    atomic::{AtomicBool, Ordering},
};

use serde::{Deserialize, Serialize};

use crate::system_health::{SystemHealthService, SystemHealthState};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LifecyclePreferences {
    pub monitoring_paused: bool,
    #[serde(default = "default_retention_days")]
    pub retention_days: u32,
    pub locale: Locale,
    pub theme: Theme,
    pub show_in_dock: bool,
    pub launch_at_login: bool,
}

fn default_retention_days() -> u32 {
    90
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
            retention_days: default_retention_days(),
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

    pub fn set_retention_days(&self, retention_days: u32) -> Result<LifecyclePreferences, String> {
        if !(1..=3650).contains(&retention_days) {
            return Err("retention days must be between 1 and 3650".to_string());
        }
        self.update(|preferences| preferences.retention_days = retention_days)
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
            if self.sampling_was_paused.swap(false, Ordering::AcqRel) {
                health.reset_rate_baseline();
            }
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
    fn retention_period_is_validated_and_persisted_at_the_preferences_seam() {
        let store = Arc::new(MemoryPreferenceStore::default());
        let lifecycle = LifecycleService::new(store.clone()).unwrap();

        assert_eq!(
            lifecycle.set_retention_days(0),
            Err("retention days must be between 1 and 3650".to_string())
        );
        assert_eq!(lifecycle.set_retention_days(30).unwrap().retention_days, 30);

        let restarted = LifecycleService::new(store).unwrap();
        assert_eq!(restarted.preferences().retention_days, 30);
    }
}
