use std::{
    collections::VecDeque,
    sync::{Arc, Mutex, RwLock},
};

use chrono::{DateTime, Duration as ChronoDuration, Timelike, Utc};
use serde::Serialize;

#[derive(Debug, Clone, PartialEq)]
pub struct MetricSnapshot {
    pub observed_at: DateTime<Utc>,
    pub cpu_percent: f32,
    pub memory_used_bytes: u64,
    pub memory_total_bytes: u64,
    pub memory_pressure: MemoryPressureLevel,
    pub disk_available_bytes: u64,
    pub disk_total_bytes: u64,
    pub network_received_bytes: u64,
    pub network_transmitted_bytes: u64,
    pub battery_percent: Option<f32>,
    pub battery_charging: Option<bool>,
    pub uptime_seconds: u64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MemoryPressureLevel {
    Normal,
    Warning,
    Critical,
}

impl MemoryPressureLevel {
    fn as_str(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Warning => "warning",
            Self::Critical => "critical",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemHealthMetrics {
    pub cpu_percent: f32,
    pub memory_used_bytes: u64,
    pub memory_total_bytes: u64,
    pub memory_pressure: String,
    pub disk_available_bytes: u64,
    pub disk_total_bytes: u64,
    pub network_down_bytes_per_second: f64,
    pub network_up_bytes_per_second: f64,
    pub battery_percent: Option<f32>,
    pub battery_charging: Option<bool>,
    pub uptime_seconds: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum SystemHealthState {
    Loading,
    Ready {
        updated_at: DateTime<Utc>,
        metrics: SystemHealthMetrics,
    },
    Stale {
        updated_at: DateTime<Utc>,
        metrics: SystemHealthMetrics,
        reason: StaleReason,
    },
    Error {
        updated_at: DateTime<Utc>,
        message: String,
        last_metrics: Option<SystemHealthMetrics>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum StaleReason {
    Paused,
    Outdated,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemHealthPoint {
    pub observed_at: DateTime<Utc>,
    pub metrics: SystemHealthMetrics,
}

pub trait MetricSource: Send + Sync {
    fn collect(&self) -> Result<MetricSnapshot, String>;
}

pub struct SystemHealthService {
    source: Arc<dyn MetricSource>,
    sampling: Mutex<()>,
    state: RwLock<SystemHealthState>,
    previous: RwLock<Option<MetricSnapshot>>,
    history: RwLock<VecDeque<SystemHealthPoint>>,
}

impl SystemHealthService {
    pub fn new(source: Arc<dyn MetricSource>) -> Self {
        Self {
            source,
            sampling: Mutex::new(()),
            state: RwLock::new(SystemHealthState::Loading),
            previous: RwLock::new(None),
            history: RwLock::new(VecDeque::with_capacity(1_800)),
        }
    }

    pub fn latest(&self) -> SystemHealthState {
        self.state.read().expect("health state poisoned").clone()
    }

    pub fn reset_rate_baseline(&self) {
        let _sampling = self.sampling.lock().expect("sampling lock poisoned");
        *self.previous.write().expect("previous sample poisoned") = None;
    }

    pub fn history(&self) -> Vec<SystemHealthPoint> {
        self.history_at(Utc::now())
    }

    pub fn clear_history(&self) {
        self.history
            .write()
            .expect("health history poisoned")
            .clear();
    }

    fn history_at(&self, now: DateTime<Utc>) -> Vec<SystemHealthPoint> {
        let mut history = self.history.write().expect("health history poisoned");
        let cutoff = now - ChronoDuration::hours(1);
        while history
            .front()
            .is_some_and(|point| point.observed_at < cutoff)
        {
            history.pop_front();
        }
        let mut aggregates: Vec<(SystemHealthPoint, u64)> = Vec::new();
        for point in history.iter() {
            let bucket = point
                .observed_at
                .with_second(0)
                .and_then(|value| value.with_nanosecond(0))
                .unwrap_or(point.observed_at);
            if let Some((aggregate, count)) = aggregates.last_mut()
                && aggregate.observed_at == bucket
            {
                let next_count = *count + 1;
                let average =
                    |current: f64, next: f64| (current * *count as f64 + next) / next_count as f64;
                aggregate.metrics.cpu_percent = average(
                    aggregate.metrics.cpu_percent as f64,
                    point.metrics.cpu_percent as f64,
                ) as f32;
                aggregate.metrics.memory_used_bytes = average(
                    aggregate.metrics.memory_used_bytes as f64,
                    point.metrics.memory_used_bytes as f64,
                ) as u64;
                aggregate.metrics.network_down_bytes_per_second = average(
                    aggregate.metrics.network_down_bytes_per_second,
                    point.metrics.network_down_bytes_per_second,
                );
                aggregate.metrics.network_up_bytes_per_second = average(
                    aggregate.metrics.network_up_bytes_per_second,
                    point.metrics.network_up_bytes_per_second,
                );
                aggregate.metrics.memory_total_bytes = point.metrics.memory_total_bytes;
                aggregate.metrics.memory_pressure = point.metrics.memory_pressure.clone();
                aggregate.metrics.disk_available_bytes = point.metrics.disk_available_bytes;
                aggregate.metrics.disk_total_bytes = point.metrics.disk_total_bytes;
                aggregate.metrics.battery_percent = point.metrics.battery_percent;
                aggregate.metrics.battery_charging = point.metrics.battery_charging;
                aggregate.metrics.uptime_seconds = point.metrics.uptime_seconds;
                *count = next_count;
            } else {
                aggregates.push((
                    SystemHealthPoint {
                        observed_at: bucket,
                        metrics: point.metrics.clone(),
                    },
                    1,
                ));
            }
        }
        aggregates.into_iter().map(|(point, _)| point).collect()
    }

    pub fn report_error(&self, message: String) {
        let last_metrics = match self.latest() {
            SystemHealthState::Ready { metrics, .. }
            | SystemHealthState::Stale { metrics, .. }
            | SystemHealthState::Error {
                last_metrics: Some(metrics),
                ..
            } => Some(metrics),
            _ => None,
        };
        *self.state.write().expect("health state poisoned") = SystemHealthState::Error {
            updated_at: Utc::now(),
            message,
            last_metrics,
        };
    }

    pub fn sample(&self) -> Result<SystemHealthState, String> {
        let _sampling = self.sampling.lock().expect("sampling lock poisoned");
        let snapshot = match self.source.collect() {
            Ok(snapshot) => snapshot,
            Err(message) => {
                self.report_error(message.clone());
                return Err(message);
            }
        };

        let previous = self
            .previous
            .read()
            .expect("previous sample poisoned")
            .clone();
        let elapsed_seconds = previous
            .as_ref()
            .map(|value| {
                (snapshot.observed_at - value.observed_at).num_milliseconds() as f64 / 1_000.0
            })
            .filter(|seconds| *seconds > 0.0 && *seconds <= 5.0);
        let rate = |current: u64, prior: Option<u64>| match (prior, elapsed_seconds) {
            (Some(prior), Some(seconds)) if current >= prior => (current - prior) as f64 / seconds,
            _ => 0.0,
        };
        let metrics = SystemHealthMetrics {
            cpu_percent: snapshot.cpu_percent.clamp(0.0, 100.0),
            memory_used_bytes: snapshot.memory_used_bytes,
            memory_total_bytes: snapshot.memory_total_bytes,
            memory_pressure: snapshot.memory_pressure.as_str().to_string(),
            disk_available_bytes: snapshot.disk_available_bytes,
            disk_total_bytes: snapshot.disk_total_bytes,
            network_down_bytes_per_second: rate(
                snapshot.network_received_bytes,
                previous.as_ref().map(|value| value.network_received_bytes),
            ),
            network_up_bytes_per_second: rate(
                snapshot.network_transmitted_bytes,
                previous
                    .as_ref()
                    .map(|value| value.network_transmitted_bytes),
            ),
            battery_percent: snapshot.battery_percent,
            battery_charging: snapshot.battery_charging,
            uptime_seconds: snapshot.uptime_seconds,
        };
        let ready_state = SystemHealthState::Ready {
            updated_at: snapshot.observed_at,
            metrics: metrics.clone(),
        };
        let mut history = self.history.write().expect("health history poisoned");
        let cutoff = snapshot.observed_at - ChronoDuration::hours(1);
        while history
            .front()
            .is_some_and(|point| point.observed_at < cutoff)
        {
            history.pop_front();
        }
        history.push_back(SystemHealthPoint {
            observed_at: snapshot.observed_at,
            metrics,
        });
        drop(history);
        *self.previous.write().expect("previous sample poisoned") = Some(snapshot);
        *self.state.write().expect("health state poisoned") = ready_state.clone();
        Ok(ready_state)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use chrono::{TimeZone, Utc};

    use super::*;

    struct SequenceSource {
        snapshots: Mutex<Vec<MetricSnapshot>>,
    }

    impl MetricSource for SequenceSource {
        fn collect(&self) -> Result<MetricSnapshot, String> {
            Ok(self.snapshots.lock().unwrap().remove(0))
        }
    }

    fn snapshot(second: u32, received: u64, transmitted: u64) -> MetricSnapshot {
        MetricSnapshot {
            observed_at: Utc.with_ymd_and_hms(2026, 7, 26, 10, 0, second).unwrap(),
            cpu_percent: 12.5,
            memory_used_bytes: 12,
            memory_total_bytes: 100,
            memory_pressure: MemoryPressureLevel::Normal,
            disk_available_bytes: 400,
            disk_total_bytes: 1_000,
            network_received_bytes: received,
            network_transmitted_bytes: transmitted,
            battery_percent: Some(88.0),
            battery_charging: Some(false),
            uptime_seconds: 7_200,
        }
    }

    #[test]
    fn reports_loading_until_the_first_sample() {
        let source = Arc::new(SequenceSource {
            snapshots: Mutex::new(vec![]),
        });
        let service = SystemHealthService::new(source);

        assert_eq!(service.latest(), SystemHealthState::Loading);
    }

    #[test]
    fn publishes_ui_ready_metrics_and_network_rates() {
        let source = Arc::new(SequenceSource {
            snapshots: Mutex::new(vec![snapshot(0, 1_000, 2_000), snapshot(2, 3_000, 3_000)]),
        });
        let service = SystemHealthService::new(source);

        service.sample().unwrap();
        let state = service.sample().unwrap();

        let SystemHealthState::Ready { metrics, .. } = state else {
            panic!("expected ready state");
        };
        assert_eq!(metrics.network_down_bytes_per_second, 1_000.0);
        assert_eq!(metrics.network_up_bytes_per_second, 500.0);
        assert_eq!(metrics.memory_pressure, "normal");
        assert_eq!(metrics.battery_percent, Some(88.0));
        let now = Utc.with_ymd_and_hms(2026, 7, 26, 10, 0, 2).unwrap();
        assert_eq!(service.history_at(now).len(), 1);
    }

    #[test]
    fn prunes_history_by_time_and_ignores_network_deltas_across_long_gaps() {
        let mut old = snapshot(0, 1_000, 2_000);
        old.observed_at = Utc.with_ymd_and_hms(2026, 7, 26, 8, 59, 0).unwrap();
        let mut recent = snapshot(0, 9_000, 8_000);
        recent.observed_at = Utc.with_ymd_and_hms(2026, 7, 26, 10, 0, 0).unwrap();
        let source = Arc::new(SequenceSource {
            snapshots: Mutex::new(vec![old, recent]),
        });
        let service = SystemHealthService::new(source);

        service.sample().unwrap();
        let state = service.sample().unwrap();

        let SystemHealthState::Ready { metrics, .. } = state else {
            panic!("expected ready state");
        };
        assert_eq!(metrics.network_down_bytes_per_second, 0.0);
        assert_eq!(metrics.network_up_bytes_per_second, 0.0);
        let now = Utc.with_ymd_and_hms(2026, 7, 26, 10, 0, 0).unwrap();
        assert_eq!(service.history_at(now).len(), 1);
    }

    #[test]
    fn clear_history_removes_the_observable_ring_without_hiding_the_live_metric() {
        let source = Arc::new(SequenceSource {
            snapshots: Mutex::new(vec![snapshot(0, 1_000, 2_000)]),
        });
        let service = SystemHealthService::new(source);
        service.sample().unwrap();

        service.clear_history();

        assert!(service.history().is_empty());
        assert!(matches!(service.latest(), SystemHealthState::Ready { .. }));
    }
}
