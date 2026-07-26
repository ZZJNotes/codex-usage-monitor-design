use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex},
};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tauri::AppHandle;
use tauri_plugin_notification::NotificationExt;

use crate::{
    quota::{AccountId, QuotaErrorReason, QuotaSnapshot, QuotaState},
    system_health::SystemHealthState,
};

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NotificationPolicy {
    pub enabled: bool,
    pub quota_thresholds: Vec<u8>,
    pub disk_available_percent_threshold: u8,
    pub consecutive_refresh_failures: u32,
}

impl Default for NotificationPolicy {
    fn default() -> Self {
        Self {
            enabled: true,
            quota_thresholds: vec![20, 10, 0],
            disk_available_percent_threshold: 10,
            consecutive_refresh_failures: 3,
        }
    }
}

impl NotificationPolicy {
    pub fn validate(&self) -> Result<(), String> {
        if self.quota_thresholds.is_empty()
            || self.quota_thresholds.len() > 5
            || self
                .quota_thresholds
                .iter()
                .any(|threshold| *threshold > 100)
        {
            return Err("quota thresholds must contain 1 to 5 percentages from 0 to 100".into());
        }
        let unique = self
            .quota_thresholds
            .iter()
            .copied()
            .collect::<BTreeSet<_>>();
        if unique.len() != self.quota_thresholds.len() {
            return Err("quota thresholds must be unique".into());
        }
        if self.disk_available_percent_threshold > 100 {
            return Err("disk threshold must be a percentage from 0 to 100".into());
        }
        if self.consecutive_refresh_failures == 0 || self.consecutive_refresh_failures > 20 {
            return Err("consecutive refresh failures must be from 1 to 20".into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum NotificationLocale {
    Chinese,
    English,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SystemNotification {
    pub title: String,
    pub body: String,
}

pub trait NotificationSender: Send + Sync {
    fn send(&self, notification: &SystemNotification) -> Result<(), String>;
}

pub struct MacOsNotificationSender {
    app: AppHandle,
}

impl MacOsNotificationSender {
    pub fn new(app: AppHandle) -> Self {
        Self { app }
    }
}

impl NotificationSender for MacOsNotificationSender {
    fn send(&self, notification: &SystemNotification) -> Result<(), String> {
        self.app
            .notification()
            .builder()
            .title(&notification.title)
            .body(&notification.body)
            .show()
            .map_err(|error| format!("system notification failed: {error}"))
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NotificationRecord {
    pub sent_at: DateTime<Utc>,
    pub title: String,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActiveNotificationCondition {
    pub key: String,
    pub label: String,
    pub account_id: Option<AccountId>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct NotificationStatus {
    pub active_conditions: Vec<ActiveNotificationCondition>,
    pub last_notification: Option<NotificationRecord>,
    pub delivery_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, Default)]
pub struct PersistedNotificationState {
    quota_levels: BTreeMap<String, u8>,
    accounts: BTreeMap<String, AccountNotificationState>,
    disk_active: bool,
    memory_active: bool,
    status: NotificationStatus,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, Default)]
struct AccountNotificationState {
    authentication_active: bool,
    refresh_expired_active: bool,
    consecutive_refresh_failures: u32,
    last_refresh_failure_at: Option<DateTime<Utc>>,
}

pub trait NotificationStore: Send + Sync {
    fn load_notification_state(&self) -> Result<Option<PersistedNotificationState>, String>;
    fn save_notification_state(&self, state: &PersistedNotificationState) -> Result<(), String>;
}

pub trait NotificationClock: Send + Sync {
    fn now(&self) -> DateTime<Utc>;
}

struct SystemClock;

impl NotificationClock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

pub struct NotificationService {
    store: Arc<dyn NotificationStore>,
    sender: Arc<dyn NotificationSender>,
    clock: Arc<dyn NotificationClock>,
    state: Mutex<PersistedNotificationState>,
}

impl NotificationService {
    pub fn new(
        store: Arc<dyn NotificationStore>,
        sender: Arc<dyn NotificationSender>,
    ) -> Result<Self, String> {
        Self::with_dependencies(store, sender, Arc::new(SystemClock))
    }

    pub fn with_dependencies(
        store: Arc<dyn NotificationStore>,
        sender: Arc<dyn NotificationSender>,
        clock: Arc<dyn NotificationClock>,
    ) -> Result<Self, String> {
        let state = store.load_notification_state()?.unwrap_or_default();
        Ok(Self {
            store,
            sender,
            clock,
            state: Mutex::new(state),
        })
    }

    pub fn status(&self) -> NotificationStatus {
        self.state
            .lock()
            .expect("notification state poisoned")
            .status
            .clone()
    }

    pub fn evaluate_account(
        &self,
        account_id: &AccountId,
        display_name: &str,
        quota_state: &QuotaState,
        policy: &NotificationPolicy,
        locale: NotificationLocale,
    ) -> Result<(), String> {
        policy.validate()?;
        if !policy.enabled {
            return self.clear_active_conditions();
        }
        let account_key = account_id.as_str().to_string();
        let mut state = self.state.lock().expect("notification state poisoned");
        let mut pending = Vec::new();
        let account = state.accounts.entry(account_key.clone()).or_default();

        match quota_state {
            QuotaState::Ready { snapshot, .. } => {
                account.authentication_active = false;
                account.refresh_expired_active = false;
                account.consecutive_refresh_failures = 0;
                account.last_refresh_failure_at = None;
                evaluate_quota_windows(
                    &mut state.quota_levels,
                    account_id,
                    display_name,
                    snapshot,
                    policy,
                    locale,
                    &mut pending,
                );
            }
            QuotaState::Stale {
                reason,
                snapshot,
                failed_at,
                ..
            } => {
                count_refresh_failure(account, *reason, *failed_at);
                if account.consecutive_refresh_failures >= policy.consecutive_refresh_failures
                    && !account.refresh_expired_active
                {
                    account.refresh_expired_active = true;
                    pending.push(refresh_expired_notification(
                        display_name,
                        account.consecutive_refresh_failures,
                        locale,
                    ));
                }
                evaluate_quota_windows(
                    &mut state.quota_levels,
                    account_id,
                    display_name,
                    snapshot,
                    policy,
                    locale,
                    &mut pending,
                );
            }
            QuotaState::Error {
                reason,
                last_snapshot,
                failed_at,
                ..
            } => {
                if *reason == QuotaErrorReason::Reauthorization {
                    account.consecutive_refresh_failures = 0;
                    account.last_refresh_failure_at = None;
                    account.refresh_expired_active = false;
                    if !account.authentication_active {
                        account.authentication_active = true;
                        pending.push(authentication_notification(display_name, locale));
                    }
                } else {
                    count_refresh_failure(account, *reason, *failed_at);
                    if account.consecutive_refresh_failures >= policy.consecutive_refresh_failures
                        && !account.refresh_expired_active
                    {
                        account.refresh_expired_active = true;
                        pending.push(refresh_expired_notification(
                            display_name,
                            account.consecutive_refresh_failures,
                            locale,
                        ));
                    }
                }
                if let Some(snapshot) = last_snapshot {
                    evaluate_quota_windows(
                        &mut state.quota_levels,
                        account_id,
                        display_name,
                        snapshot,
                        policy,
                        locale,
                        &mut pending,
                    );
                }
            }
            QuotaState::Loading | QuotaState::Cooldown { .. } => {}
        }
        rebuild_active_conditions(&mut state, policy);
        self.deliver_and_save(&mut state, pending)
    }

    pub fn evaluate_system(
        &self,
        health_state: &SystemHealthState,
        policy: &NotificationPolicy,
        locale: NotificationLocale,
    ) -> Result<(), String> {
        policy.validate()?;
        if !policy.enabled {
            return self.clear_active_conditions();
        }
        let SystemHealthState::Ready { metrics, .. } = health_state else {
            return Ok(());
        };
        let mut state = self.state.lock().expect("notification state poisoned");
        let mut pending = Vec::new();
        let disk_available_percent = if metrics.disk_total_bytes == 0 {
            None
        } else {
            Some(
                ((metrics.disk_available_bytes as u128 * 100) / metrics.disk_total_bytes as u128)
                    as u8,
            )
        };
        let disk_exhausted = disk_available_percent
            .is_some_and(|percent| percent <= policy.disk_available_percent_threshold);
        if disk_exhausted && !state.disk_active {
            state.disk_active = true;
            pending.push(disk_notification(
                disk_available_percent.unwrap_or_default(),
                policy.disk_available_percent_threshold,
                locale,
            ));
        } else if !disk_exhausted {
            state.disk_active = false;
        }
        let memory_critical = metrics.memory_pressure == "critical";
        if memory_critical && !state.memory_active {
            state.memory_active = true;
            pending.push(memory_notification(locale));
        } else if !memory_critical {
            state.memory_active = false;
        }
        rebuild_active_conditions(&mut state, policy);
        self.deliver_and_save(&mut state, pending)
    }

    fn deliver_and_save(
        &self,
        state: &mut PersistedNotificationState,
        pending: Vec<SystemNotification>,
    ) -> Result<(), String> {
        for notification in pending {
            match self.sender.send(&notification) {
                Ok(()) => {
                    state.status.last_notification = Some(NotificationRecord {
                        sent_at: self.clock.now(),
                        title: notification.title,
                        body: notification.body,
                    });
                    state.status.delivery_error = None;
                }
                Err(error) => state.status.delivery_error = Some(error),
            }
        }
        self.store.save_notification_state(state)
    }

    fn clear_active_conditions(&self) -> Result<(), String> {
        let mut state = self.state.lock().expect("notification state poisoned");
        state.quota_levels.clear();
        state.accounts.clear();
        state.disk_active = false;
        state.memory_active = false;
        state.status.active_conditions.clear();
        self.store.save_notification_state(&state)
    }
}

fn count_refresh_failure(
    account: &mut AccountNotificationState,
    reason: QuotaErrorReason,
    failed_at: DateTime<Utc>,
) {
    if !matches!(
        reason,
        QuotaErrorReason::Transport | QuotaErrorReason::Service | QuotaErrorReason::InvalidResponse
    ) {
        return;
    }
    if account.last_refresh_failure_at != Some(failed_at) {
        account.consecutive_refresh_failures =
            account.consecutive_refresh_failures.saturating_add(1);
        account.last_refresh_failure_at = Some(failed_at);
    }
}

#[allow(clippy::too_many_arguments)]
fn evaluate_quota_windows(
    active_levels: &mut BTreeMap<String, u8>,
    account_id: &AccountId,
    display_name: &str,
    snapshot: &QuotaSnapshot,
    policy: &NotificationPolicy,
    locale: NotificationLocale,
    pending: &mut Vec<SystemNotification>,
) {
    let prefix = format!("{}\u{1f}", account_id.as_str());
    let observed_keys = snapshot
        .windows
        .iter()
        .map(|window| format!("{prefix}{}", window.name))
        .collect::<BTreeSet<_>>();
    active_levels.retain(|key, _| !key.starts_with(&prefix) || observed_keys.contains(key));
    let recovery_threshold = policy
        .quota_thresholds
        .iter()
        .copied()
        .max()
        .unwrap_or_default();
    for window in &snapshot.windows {
        let key = format!("{prefix}{}", window.name);
        if window.remaining_percent > recovery_threshold {
            active_levels.remove(&key);
            continue;
        }
        let current_level = policy
            .quota_thresholds
            .iter()
            .copied()
            .filter(|threshold| window.remaining_percent <= *threshold)
            .min();
        let Some(current_level) = current_level else {
            continue;
        };
        let should_send = active_levels
            .get(&key)
            .is_none_or(|previous| current_level < *previous);
        if should_send {
            active_levels.insert(key, current_level);
            pending.push(quota_notification(
                display_name,
                &window.name,
                window.remaining_percent,
                locale,
            ));
        }
    }
}

fn rebuild_active_conditions(state: &mut PersistedNotificationState, policy: &NotificationPolicy) {
    let mut conditions = Vec::new();
    for (key, threshold) in &state.quota_levels {
        let (account_id, window) = key.split_once('\u{1f}').unwrap_or((key, "quota"));
        conditions.push(ActiveNotificationCondition {
            key: format!("quota:{key}"),
            label: format!("{window} ≤ {threshold}%"),
            account_id: Some(AccountId::from(account_id)),
        });
    }
    for (account_id, account) in &state.accounts {
        if account.authentication_active {
            conditions.push(ActiveNotificationCondition {
                key: format!("authentication:{account_id}"),
                label: "OAuth reauthorization required".into(),
                account_id: Some(AccountId::from(account_id.as_str())),
            });
        }
        if account.refresh_expired_active {
            conditions.push(ActiveNotificationCondition {
                key: format!("refresh:{account_id}"),
                label: format!(
                    "Quota refresh failed {} consecutive times",
                    policy.consecutive_refresh_failures
                ),
                account_id: Some(AccountId::from(account_id.as_str())),
            });
        }
    }
    if state.disk_active {
        conditions.push(ActiveNotificationCondition {
            key: "disk".into(),
            label: format!(
                "Disk available space ≤ {}%",
                policy.disk_available_percent_threshold
            ),
            account_id: None,
        });
    }
    if state.memory_active {
        conditions.push(ActiveNotificationCondition {
            key: "memoryPressure".into(),
            label: "Memory pressure is critical".into(),
            account_id: None,
        });
    }
    state.status.active_conditions = conditions;
}

fn quota_notification(
    account: &str,
    window: &str,
    remaining: u8,
    locale: NotificationLocale,
) -> SystemNotification {
    match locale {
        NotificationLocale::Chinese => SystemNotification {
            title: format!("{account} 额度提醒"),
            body: format!(
                "原因：{window} 仅剩 {remaining}%。影响：继续使用可能耗尽该额度窗口。恢复：查看重置时间，或改用已独立授权且有可用额度的账户。"
            ),
        },
        NotificationLocale::English => SystemNotification {
            title: format!("Quota alert for {account}"),
            body: format!(
                "Reason: {window} has {remaining}% remaining. Impact: continued use may exhaust this quota window. Recovery: check its reset time or use an independently authorized account with available quota."
            ),
        },
    }
}

fn authentication_notification(account: &str, locale: NotificationLocale) -> SystemNotification {
    match locale {
        NotificationLocale::Chinese => SystemNotification {
            title: format!("{account} 需要重新授权"),
            body: "原因：OAuth 授权已失效。影响：该账户的额度无法继续刷新。恢复：在账户管理中只重新授权这个账户。".into(),
        },
        NotificationLocale::English => SystemNotification {
            title: format!("Reauthorize {account}"),
            body: "Reason: OAuth authorization expired. Impact: quota for this account cannot refresh. Recovery: reauthorize only this account in account management.".into(),
        },
    }
}

fn refresh_expired_notification(
    account: &str,
    failures: u32,
    locale: NotificationLocale,
) -> SystemNotification {
    match locale {
        NotificationLocale::Chinese => SystemNotification {
            title: format!("{account} 额度数据持续过期"),
            body: format!(
                "原因：额度刷新已连续失败 {failures} 次。影响：当前显示的是最后可信快照，而不是实时额度。恢复：检查网络或上游服务；应用会按退避策略重试。"
            ),
        },
        NotificationLocale::English => SystemNotification {
            title: format!("Quota data expired for {account}"),
            body: format!(
                "Reason: quota refresh failed {failures} consecutive times. Impact: the app is showing the last trusted snapshot, not live quota. Recovery: check the network or upstream service; the app will retry with backoff."
            ),
        },
    }
}

fn disk_notification(
    available: u8,
    threshold: u8,
    locale: NotificationLocale,
) -> SystemNotification {
    match locale {
        NotificationLocale::Chinese => SystemNotification {
            title: "磁盘可用空间不足".into(),
            body: format!(
                "原因：磁盘可用空间为 {available}%，已达到 {threshold}% 阈值。影响：本地快照、会话统计或应用运行可能失败。恢复：释放磁盘空间，使可用空间高于阈值。"
            ),
        },
        NotificationLocale::English => SystemNotification {
            title: "Low disk space".into(),
            body: format!(
                "Reason: disk space is at {available}% available, reaching the {threshold}% threshold. Impact: local snapshots, session statistics, or the app may fail. Recovery: free disk space until availability is above the threshold."
            ),
        },
    }
}

fn memory_notification(locale: NotificationLocale) -> SystemNotification {
    match locale {
        NotificationLocale::Chinese => SystemNotification {
            title: "内存压力严重".into(),
            body: "原因：macOS 内存压力已达到严重级别。影响：应用和系统可能明显变慢或被终止。恢复：关闭不需要的高内存任务，直到内存压力恢复。".into(),
        },
        NotificationLocale::English => SystemNotification {
            title: "Critical memory pressure".into(),
            body: "Reason: macOS memory pressure is critical. Impact: apps and the system may slow down or be terminated. Recovery: close unneeded memory-intensive work until pressure recovers.".into(),
        },
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use chrono::{Duration, TimeZone};

    use super::*;
    use crate::{
        quota::{QuotaAccount, QuotaWindow},
        system_health::SystemHealthMetrics,
    };

    #[derive(Default)]
    struct MemoryStore(Mutex<Option<PersistedNotificationState>>);

    impl NotificationStore for MemoryStore {
        fn load_notification_state(&self) -> Result<Option<PersistedNotificationState>, String> {
            Ok(self.0.lock().unwrap().clone())
        }

        fn save_notification_state(
            &self,
            state: &PersistedNotificationState,
        ) -> Result<(), String> {
            *self.0.lock().unwrap() = Some(state.clone());
            Ok(())
        }
    }

    #[derive(Default)]
    struct FakeSender(Mutex<Vec<SystemNotification>>);

    impl NotificationSender for FakeSender {
        fn send(&self, notification: &SystemNotification) -> Result<(), String> {
            self.0.lock().unwrap().push(notification.clone());
            Ok(())
        }
    }

    struct FixedClock(DateTime<Utc>);

    impl NotificationClock for FixedClock {
        fn now(&self) -> DateTime<Utc> {
            self.0
        }
    }

    fn time(minute: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 7, 27, 10, minute, 0).unwrap()
    }

    fn quota_state(remaining_percent: u8, observed_at: DateTime<Utc>) -> QuotaState {
        QuotaState::Ready {
            snapshot: QuotaSnapshot {
                account: QuotaAccount {
                    id: "account-a".into(),
                    display_name: "Account A".to_string(),
                    plan_type: "plus".to_string(),
                },
                windows: vec![QuotaWindow {
                    name: "codex · primary".to_string(),
                    remaining_percent,
                    resets_at: None,
                    window_duration_minutes: Some(300),
                }],
                updated_at: observed_at,
            },
            next_refresh_at: observed_at + Duration::minutes(10),
        }
    }

    fn health_state(memory_pressure: &str, disk_available: u64) -> SystemHealthState {
        SystemHealthState::Ready {
            updated_at: time(0),
            metrics: SystemHealthMetrics {
                cpu_percent: 99.0,
                memory_used_bytes: 90,
                memory_total_bytes: 100,
                memory_pressure: memory_pressure.to_string(),
                disk_available_bytes: disk_available,
                disk_total_bytes: 1_000,
                network_down_bytes_per_second: 999_999.0,
                network_up_bytes_per_second: 999_999.0,
                battery_percent: None,
                battery_charging: None,
                uptime_seconds: 1,
            },
        }
    }

    fn service(sender: Arc<FakeSender>) -> NotificationService {
        NotificationService::with_dependencies(
            Arc::new(MemoryStore::default()),
            sender,
            Arc::new(FixedClock(time(0))),
        )
        .unwrap()
    }

    #[test]
    fn quota_thresholds_escalate_once_and_reset_after_recovery_per_account() {
        let sender = Arc::new(FakeSender::default());
        let service = service(sender.clone());
        let account_a = AccountId::from("account-a");
        let policy = NotificationPolicy::default();

        for remaining in [20, 20, 10, 10, 0, 0, 21, 20] {
            service
                .evaluate_account(
                    &account_a,
                    "Account A",
                    &quota_state(remaining, time(0)),
                    &policy,
                    NotificationLocale::English,
                )
                .unwrap();
        }

        let notifications = sender.0.lock().unwrap();
        assert_eq!(notifications.len(), 4);
        assert!(notifications[0].body.contains("20% remaining"));
        assert!(notifications[1].body.contains("10% remaining"));
        assert!(notifications[2].body.contains("0% remaining"));
        assert!(notifications[3].body.contains("20% remaining"));
        assert!(
            notifications
                .iter()
                .all(|item| { item.body.contains("Impact:") && item.body.contains("Recovery:") })
        );
    }

    #[test]
    fn authentication_and_three_distinct_refresh_failures_notify_until_recovered() {
        let sender = Arc::new(FakeSender::default());
        let service = service(sender.clone());
        let account = AccountId::from("account-a");
        let policy = NotificationPolicy::default();
        let auth = QuotaState::Error {
            reason: QuotaErrorReason::Reauthorization,
            last_snapshot: None,
            failed_at: time(0),
            retry_at: None,
        };

        service
            .evaluate_account(
                &account,
                "Account A",
                &auth,
                &policy,
                NotificationLocale::English,
            )
            .unwrap();
        service
            .evaluate_account(
                &account,
                "Account A",
                &auth,
                &policy,
                NotificationLocale::English,
            )
            .unwrap();
        service
            .evaluate_account(
                &account,
                "Account A",
                &quota_state(80, time(1)),
                &policy,
                NotificationLocale::English,
            )
            .unwrap();
        service
            .evaluate_account(
                &account,
                "Account A",
                &auth,
                &policy,
                NotificationLocale::English,
            )
            .unwrap();

        for minute in [2, 2, 3, 4, 4] {
            let stale = QuotaState::Stale {
                reason: QuotaErrorReason::Transport,
                snapshot: match quota_state(80, time(1)) {
                    QuotaState::Ready { snapshot, .. } => snapshot,
                    _ => unreachable!(),
                },
                failed_at: time(minute),
                retry_at: time(minute) + Duration::seconds(30),
            };
            service
                .evaluate_account(
                    &account,
                    "Account A",
                    &stale,
                    &policy,
                    NotificationLocale::English,
                )
                .unwrap();
        }

        let notifications = sender.0.lock().unwrap();
        assert_eq!(notifications.len(), 3);
        assert!(notifications[0].body.contains("authorization expired"));
        assert!(notifications[1].body.contains("authorization expired"));
        assert!(notifications[2].body.contains("failed 3 consecutive times"));
    }

    #[test]
    fn disk_and_critical_memory_notify_once_but_cpu_and_network_do_not() {
        let sender = Arc::new(FakeSender::default());
        let service = service(sender.clone());
        let policy = NotificationPolicy::default();

        for state in [
            health_state("normal", 500),
            health_state("critical", 500),
            health_state("critical", 500),
            health_state("normal", 500),
            health_state("critical", 500),
            health_state("normal", 100),
            health_state("normal", 100),
            health_state("normal", 110),
            health_state("normal", 100),
        ] {
            service
                .evaluate_system(&state, &policy, NotificationLocale::English)
                .unwrap();
        }

        let notifications = sender.0.lock().unwrap();
        assert_eq!(notifications.len(), 4);
        assert_eq!(
            notifications
                .iter()
                .filter(|item| item.body.contains("memory pressure is critical"))
                .count(),
            2
        );
        assert_eq!(
            notifications
                .iter()
                .filter(|item| item.body.contains("disk space is at 10% available"))
                .count(),
            2
        );
    }

    #[test]
    fn persisted_active_condition_suppresses_duplicates_after_restart() {
        let store = Arc::new(MemoryStore::default());
        let sender = Arc::new(FakeSender::default());
        let policy = NotificationPolicy::default();
        let account = AccountId::from("account-a");
        let first = NotificationService::with_dependencies(
            store.clone(),
            sender.clone(),
            Arc::new(FixedClock(time(0))),
        )
        .unwrap();
        first
            .evaluate_account(
                &account,
                "Account A",
                &quota_state(20, time(0)),
                &policy,
                NotificationLocale::English,
            )
            .unwrap();
        let restarted = NotificationService::with_dependencies(
            store,
            sender.clone(),
            Arc::new(FixedClock(time(1))),
        )
        .unwrap();

        restarted
            .evaluate_account(
                &account,
                "Account A",
                &quota_state(20, time(1)),
                &policy,
                NotificationLocale::English,
            )
            .unwrap();

        assert_eq!(sender.0.lock().unwrap().len(), 1);
        assert_eq!(restarted.status().active_conditions.len(), 1);
    }

    #[test]
    fn recovery_while_notifications_are_disabled_allows_a_future_alert() {
        let sender = Arc::new(FakeSender::default());
        let service = service(sender.clone());
        let account = AccountId::from("account-a");
        let enabled = NotificationPolicy::default();
        let mut disabled = enabled.clone();
        disabled.enabled = false;

        service
            .evaluate_account(
                &account,
                "Account A",
                &quota_state(20, time(0)),
                &enabled,
                NotificationLocale::English,
            )
            .unwrap();
        service
            .evaluate_account(
                &account,
                "Account A",
                &quota_state(80, time(1)),
                &disabled,
                NotificationLocale::English,
            )
            .unwrap();
        service
            .evaluate_account(
                &account,
                "Account A",
                &quota_state(20, time(2)),
                &enabled,
                NotificationLocale::English,
            )
            .unwrap();

        assert_eq!(sender.0.lock().unwrap().len(), 2);
    }
}
