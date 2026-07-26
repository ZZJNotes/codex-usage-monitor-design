use std::{
    process::Command,
    sync::Mutex,
    time::{Duration, Instant},
};

use chrono::Utc;
use sysinfo::{Disks, Networks, System};

use crate::system_health::{MemoryPressureLevel, MetricSnapshot, MetricSource};

pub struct MacMetricSource {
    collector: Mutex<HostCollector>,
}

struct HostCollector {
    system: System,
    disks: Disks,
    networks: Networks,
    battery: Option<CachedBattery>,
}

#[derive(Clone, Copy)]
struct CachedBattery {
    checked_at: Instant,
    percent: Option<f32>,
    charging: Option<bool>,
}

impl MacMetricSource {
    pub fn new() -> Self {
        let mut system = System::new_all();
        system.refresh_cpu_usage();
        Self {
            collector: Mutex::new(HostCollector {
                system,
                disks: Disks::new_with_refreshed_list(),
                networks: Networks::new_with_refreshed_list(),
                battery: None,
            }),
        }
    }
}

impl Default for MacMetricSource {
    fn default() -> Self {
        Self::new()
    }
}

impl MetricSource for MacMetricSource {
    fn collect(&self) -> Result<MetricSnapshot, String> {
        let mut collector = self
            .collector
            .lock()
            .map_err(|_| "metric collector lock poisoned")?;
        collector.system.refresh_cpu_usage();
        collector.system.refresh_memory();
        collector.disks.refresh(true);
        collector.networks.refresh(true);

        let root_disk = collector
            .disks
            .iter()
            .find(|disk| disk.mount_point().to_string_lossy() == "/")
            .or_else(|| collector.disks.iter().max_by_key(|disk| disk.total_space()));
        let (disk_available_bytes, disk_total_bytes) = root_disk
            .map(|disk| (disk.available_space(), disk.total_space()))
            .unwrap_or((0, 0));
        let network_received_bytes = collector
            .networks
            .iter()
            .filter(|(name, _)| name.as_str() != "lo0")
            .map(|(_, data)| data.total_received())
            .sum();
        let network_transmitted_bytes = collector
            .networks
            .iter()
            .filter(|(name, _)| name.as_str() != "lo0")
            .map(|(_, data)| data.total_transmitted())
            .sum();
        let battery = match collector.battery {
            Some(value) if value.checked_at.elapsed() < Duration::from_secs(30) => value,
            _ => {
                let (percent, charging) = read_battery();
                let value = CachedBattery {
                    checked_at: Instant::now(),
                    percent,
                    charging,
                };
                collector.battery = Some(value);
                value
            }
        };

        Ok(MetricSnapshot {
            observed_at: Utc::now(),
            cpu_percent: collector.system.global_cpu_usage(),
            memory_used_bytes: collector.system.used_memory(),
            memory_total_bytes: collector.system.total_memory(),
            memory_pressure: read_memory_pressure(),
            disk_available_bytes,
            disk_total_bytes,
            network_received_bytes,
            network_transmitted_bytes,
            battery_percent: battery.percent,
            battery_charging: battery.charging,
            uptime_seconds: System::uptime(),
        })
    }
}

#[cfg(target_os = "macos")]
fn read_memory_pressure() -> MemoryPressureLevel {
    let name = c"kern.memorystatus_vm_pressure_level";
    let mut value: i32 = 1;
    let mut length = std::mem::size_of::<i32>();
    let result = unsafe {
        libc::sysctlbyname(
            name.as_ptr(),
            (&mut value as *mut i32).cast(),
            &mut length,
            std::ptr::null_mut(),
            0,
        )
    };
    if result != 0 {
        return MemoryPressureLevel::Normal;
    }
    match value {
        4.. => MemoryPressureLevel::Critical,
        2..=3 => MemoryPressureLevel::Warning,
        _ => MemoryPressureLevel::Normal,
    }
}

#[cfg(not(target_os = "macos"))]
fn read_memory_pressure() -> MemoryPressureLevel {
    MemoryPressureLevel::Normal
}

fn read_battery() -> (Option<f32>, Option<bool>) {
    #[cfg(target_os = "macos")]
    {
        return Command::new("/usr/bin/pmset")
            .args(["-g", "batt"])
            .output()
            .ok()
            .filter(|output| output.status.success())
            .and_then(|output| String::from_utf8(output.stdout).ok())
            .map(|output| parse_battery(&output))
            .unwrap_or((None, None));
    }
    #[allow(unreachable_code)]
    (None, None)
}

fn parse_battery(output: &str) -> (Option<f32>, Option<bool>) {
    let percent = output
        .split_whitespace()
        .find_map(|part| part.strip_suffix("%;").or_else(|| part.strip_suffix('%')))
        .and_then(|value| value.parse::<f32>().ok());
    let lower = output.to_ascii_lowercase();
    let charging = percent.map(|_| lower.contains("charging") && !lower.contains("discharging"));
    (percent, charging)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_mac_battery_status_without_exposing_command_output() {
        let output = "Now drawing from 'Battery Power'\n -InternalBattery-0 (id=1)\t88%; discharging; 5:20 remaining present: true";

        assert_eq!(parse_battery(output), (Some(88.0), Some(false)));
    }
}
