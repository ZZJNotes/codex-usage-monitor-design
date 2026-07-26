use std::{
    sync::{Arc, Mutex, RwLock, TryLockError},
    time::Duration,
};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize, Serialize)]
#[serde(transparent)]
pub struct AccountId(String);

impl AccountId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<String> for AccountId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for AccountId {
    fn from(value: &str) -> Self {
        Self(value.to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaAccount {
    pub id: AccountId,
    pub display_name: String,
    pub plan_type: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaWindow {
    pub name: String,
    pub remaining_percent: u8,
    pub resets_at: Option<DateTime<Utc>>,
    pub window_duration_minutes: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaSnapshot {
    pub account: QuotaAccount,
    pub windows: Vec<QuotaWindow>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum QuotaState {
    Loading,
    Ready {
        snapshot: QuotaSnapshot,
        next_refresh_at: DateTime<Utc>,
    },
    Stale {
        reason: QuotaErrorReason,
        snapshot: QuotaSnapshot,
        failed_at: DateTime<Utc>,
        retry_at: DateTime<Utc>,
    },
    Error {
        reason: QuotaErrorReason,
        last_snapshot: Option<QuotaSnapshot>,
        failed_at: DateTime<Utc>,
        retry_at: Option<DateTime<Utc>>,
    },
    Cooldown {
        snapshot: Option<QuotaSnapshot>,
        retry_at: DateTime<Utc>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum QuotaErrorReason {
    Paused,
    Storage,
    Reauthorization,
    Transport,
    Service,
    InvalidResponse,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum QuotaFailureKind {
    Authentication,
    Transport,
    Service,
    InvalidResponse,
}

#[derive(Debug, Clone, PartialEq)]
pub struct QuotaRefreshError {
    pub kind: QuotaFailureKind,
    pub message: String,
}

impl QuotaRefreshError {
    pub fn new(kind: QuotaFailureKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

pub trait QuotaSource: Send + Sync {
    fn refresh(&self) -> Result<QuotaSnapshot, QuotaRefreshError>;
}

pub trait QuotaStore: Send + Sync {
    fn load(&self, account_id: &AccountId) -> Result<Option<QuotaSnapshot>, String>;
    fn save(&self, account_id: &AccountId, snapshot: &QuotaSnapshot) -> Result<(), String>;
}

pub trait QuotaClock: Send + Sync {
    fn now(&self) -> DateTime<Utc>;
}

struct SystemClock;

impl QuotaClock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

#[derive(Debug, Clone, Copy)]
pub struct RefreshPolicy {
    pub interval: Duration,
    pub jitter: Duration,
    pub manual_cooldown: Duration,
    pub recovery_cooldown: Duration,
    pub backoff_base: Duration,
    pub backoff_max: Duration,
}

impl Default for RefreshPolicy {
    fn default() -> Self {
        Self {
            interval: Duration::from_secs(9 * 60),
            jitter: Duration::from_secs(2 * 60),
            manual_cooldown: Duration::from_secs(30),
            recovery_cooldown: Duration::from_secs(60),
            backoff_base: Duration::from_secs(30),
            backoff_max: Duration::from_secs(15 * 60),
        }
    }
}

#[derive(Debug)]
struct RefreshSchedule {
    next_automatic_at: DateTime<Utc>,
    manual_retry_at: Option<DateTime<Utc>>,
    last_recovery_at: Option<DateTime<Utc>>,
    consecutive_failures: u32,
    reauthorization_required: bool,
}

#[derive(Debug, Clone, Copy)]
enum RefreshTrigger {
    Automatic,
    Manual,
    Recovery,
    Evidence,
}

pub struct QuotaService {
    account_id: AccountId,
    source: Arc<dyn QuotaSource>,
    store: Option<Arc<dyn QuotaStore>>,
    clock: Arc<dyn QuotaClock>,
    policy: RefreshPolicy,
    state: RwLock<QuotaState>,
    schedule: Mutex<RefreshSchedule>,
    refresh_lock: Mutex<()>,
}

impl QuotaService {
    pub fn new(account_id: impl Into<AccountId>, source: Arc<dyn QuotaSource>) -> Self {
        Self::with_dependencies(
            account_id,
            source,
            None,
            Arc::new(SystemClock),
            RefreshPolicy::default(),
        )
    }

    pub fn with_store(
        account_id: impl Into<AccountId>,
        source: Arc<dyn QuotaSource>,
        store: Arc<dyn QuotaStore>,
    ) -> Self {
        Self::with_dependencies(
            account_id,
            source,
            Some(store),
            Arc::new(SystemClock),
            RefreshPolicy::default(),
        )
    }

    fn with_dependencies(
        account_id: impl Into<AccountId>,
        source: Arc<dyn QuotaSource>,
        store: Option<Arc<dyn QuotaStore>>,
        clock: Arc<dyn QuotaClock>,
        policy: RefreshPolicy,
    ) -> Self {
        let account_id = account_id.into();
        let now = clock.now();
        let first_refresh_at =
            now + chrono_duration(account_jitter(account_id.as_str(), policy.jitter));
        let restored_snapshot = if account_id.as_str() == CURRENT_CODEX_ACCOUNT_ID {
            None
        } else {
            store.as_ref().map(|store| store.load(&account_id))
        };
        let state = match restored_snapshot {
            Some(Ok(Some(snapshot))) => QuotaState::Ready {
                snapshot,
                next_refresh_at: first_refresh_at,
            },
            Some(Err(_)) => QuotaState::Error {
                reason: QuotaErrorReason::Storage,
                last_snapshot: None,
                failed_at: now,
                retry_at: None,
            },
            _ => QuotaState::Loading,
        };
        Self {
            account_id,
            source,
            store,
            clock,
            policy,
            state: RwLock::new(state),
            schedule: Mutex::new(RefreshSchedule {
                next_automatic_at: first_refresh_at,
                manual_retry_at: None,
                last_recovery_at: None,
                consecutive_failures: 0,
                reauthorization_required: false,
            }),
            refresh_lock: Mutex::new(()),
        }
    }

    pub fn unavailable(account_id: impl Into<AccountId>, message: String) -> Self {
        let service = Self::new(
            account_id,
            Arc::new(UnavailableQuotaSource(message.clone())),
        );
        let now = service.clock.now();
        service.complete_failure(
            now,
            QuotaRefreshError::new(QuotaFailureKind::Service, message),
        );
        service
    }

    pub fn unavailable_with_store(
        account_id: impl Into<AccountId>,
        message: String,
        store: Arc<dyn QuotaStore>,
    ) -> Self {
        let source_message = message.clone();
        let service = Self::with_store(
            account_id,
            Arc::new(UnavailableQuotaSource(source_message)),
            store,
        );
        let now = service.clock.now();
        service.complete_failure(
            now,
            QuotaRefreshError::new(QuotaFailureKind::Service, message),
        );
        service
    }

    pub fn account_id(&self) -> &AccountId {
        &self.account_id
    }

    pub fn latest(&self) -> QuotaState {
        self.state.read().expect("quota state poisoned").clone()
    }

    pub fn manual_refresh(&self) -> QuotaState {
        self.refresh(RefreshTrigger::Manual)
    }

    pub fn refresh_if_due(&self) -> QuotaState {
        self.refresh(RefreshTrigger::Automatic)
    }

    pub fn recover_if_due(&self) -> QuotaState {
        self.refresh(RefreshTrigger::Recovery)
    }

    pub(crate) fn refresh_account_evidence(&self) -> QuotaState {
        self.refresh(RefreshTrigger::Evidence)
    }

    fn stagger_recovery_if_due(&self) {
        let now = self.clock.now();
        let mut schedule = self.schedule.lock().expect("quota schedule poisoned");
        if schedule.reauthorization_required || now < schedule.next_automatic_at {
            return;
        }
        if let Some(last_recovery_at) = schedule.last_recovery_at
            && now < last_recovery_at + chrono_duration(self.policy.recovery_cooldown)
        {
            return;
        }
        schedule.last_recovery_at = Some(now);
        schedule.next_automatic_at = now
            + chrono::Duration::seconds(1)
            + chrono_duration(account_jitter(self.account_id.as_str(), self.policy.jitter));
    }

    pub fn authorization_restored(&self) -> QuotaState {
        {
            let mut schedule = self.schedule.lock().expect("quota schedule poisoned");
            schedule.reauthorization_required = false;
            schedule.next_automatic_at = self.clock.now();
            schedule.manual_retry_at = None;
        }
        self.refresh(RefreshTrigger::Recovery)
    }

    pub fn paused(&self) -> QuotaState {
        let now = self.clock.now();
        QuotaState::Error {
            reason: QuotaErrorReason::Paused,
            last_snapshot: snapshot_from_state(&self.latest()),
            failed_at: now,
            retry_at: None,
        }
    }

    fn refresh(&self, trigger: RefreshTrigger) -> QuotaState {
        let _refresh = match self.refresh_lock.try_lock() {
            Ok(lock) => lock,
            Err(TryLockError::WouldBlock) => return self.latest(),
            Err(TryLockError::Poisoned(_)) => panic!("quota refresh lock poisoned"),
        };
        let now = self.clock.now();
        {
            let mut schedule = self.schedule.lock().expect("quota schedule poisoned");
            if schedule.reauthorization_required {
                return self.latest();
            }
            match trigger {
                RefreshTrigger::Manual => {
                    if let Some(retry_at) = schedule.manual_retry_at
                        && now < retry_at
                    {
                        return QuotaState::Cooldown {
                            snapshot: snapshot_from_state(&self.latest()),
                            retry_at,
                        };
                    }
                    schedule.manual_retry_at =
                        Some(now + chrono_duration(self.policy.manual_cooldown));
                }
                RefreshTrigger::Automatic if now < schedule.next_automatic_at => {
                    return self.latest();
                }
                RefreshTrigger::Recovery => {
                    if now < schedule.next_automatic_at {
                        return self.latest();
                    }
                    if let Some(last_recovery_at) = schedule.last_recovery_at
                        && now < last_recovery_at + chrono_duration(self.policy.recovery_cooldown)
                    {
                        return self.latest();
                    }
                    schedule.last_recovery_at = Some(now);
                }
                RefreshTrigger::Automatic => {}
                RefreshTrigger::Evidence => {}
            }
        }

        match self.source.refresh() {
            Ok(snapshot) => self.complete_success(now, snapshot),
            Err(error) => self.complete_failure(now, error),
        }
    }

    fn complete_success(&self, now: DateTime<Utc>, snapshot: QuotaSnapshot) -> QuotaState {
        if self.account_id.as_str() != CURRENT_CODEX_ACCOUNT_ID
            && snapshot.account.id != self.account_id
        {
            return self.complete_failure(
                now,
                QuotaRefreshError::new(
                    QuotaFailureKind::Authentication,
                    "quota response belonged to a different account",
                ),
            );
        }
        let next_refresh_at = now
            + chrono_duration(self.policy.interval)
            + chrono_duration(account_jitter(self.account_id.as_str(), self.policy.jitter));
        {
            let mut schedule = self.schedule.lock().expect("quota schedule poisoned");
            schedule.consecutive_failures = 0;
            schedule.reauthorization_required = false;
            schedule.next_automatic_at = next_refresh_at;
        }
        let state = if let Some(store) = &self.store
            && store.save(&self.account_id, &snapshot).is_err()
        {
            QuotaState::Error {
                reason: QuotaErrorReason::Storage,
                last_snapshot: Some(snapshot),
                failed_at: now,
                retry_at: None,
            }
        } else {
            QuotaState::Ready {
                snapshot,
                next_refresh_at,
            }
        };
        self.set_state(state)
    }

    fn complete_failure(&self, now: DateTime<Utc>, error: QuotaRefreshError) -> QuotaState {
        let reason = match error.kind {
            QuotaFailureKind::Authentication => QuotaErrorReason::Reauthorization,
            QuotaFailureKind::Transport => QuotaErrorReason::Transport,
            QuotaFailureKind::Service => QuotaErrorReason::Service,
            QuotaFailureKind::InvalidResponse => QuotaErrorReason::InvalidResponse,
        };
        let retry_at = if error.kind == QuotaFailureKind::Authentication {
            self.schedule
                .lock()
                .expect("quota schedule poisoned")
                .reauthorization_required = true;
            None
        } else {
            let mut schedule = self.schedule.lock().expect("quota schedule poisoned");
            schedule.consecutive_failures = schedule.consecutive_failures.saturating_add(1);
            let exponent = schedule.consecutive_failures.saturating_sub(1).min(31);
            let multiplier = 1_u32 << exponent;
            let delay = self
                .policy
                .backoff_base
                .saturating_mul(multiplier)
                .min(self.policy.backoff_max);
            let retry_at = now + chrono_duration(delay);
            schedule.next_automatic_at = retry_at;
            schedule.manual_retry_at = Some(
                schedule
                    .manual_retry_at
                    .map_or(retry_at, |manual_retry_at| manual_retry_at.max(retry_at)),
            );
            Some(retry_at)
        };
        let last_snapshot = snapshot_from_state(&self.latest());
        let state = match (last_snapshot, retry_at) {
            (Some(snapshot), Some(retry_at)) => QuotaState::Stale {
                reason,
                snapshot,
                failed_at: now,
                retry_at,
            },
            (last_snapshot, retry_at) => QuotaState::Error {
                reason,
                last_snapshot,
                failed_at: now,
                retry_at,
            },
        };
        self.set_state(state)
    }

    fn set_state(&self, state: QuotaState) -> QuotaState {
        *self.state.write().expect("quota state poisoned") = state.clone();
        state
    }
}

pub const CURRENT_CODEX_ACCOUNT_ID: &str = "current-codex-account";

pub struct QuotaRefreshCoordinator {
    accounts: Vec<Arc<QuotaService>>,
}

impl QuotaRefreshCoordinator {
    pub fn new(accounts: Vec<Arc<QuotaService>>) -> Self {
        Self { accounts }
    }

    pub fn refresh_due(&self) {
        for account in &self.accounts {
            account.refresh_if_due();
        }
    }

    pub fn stagger_due_recoveries(&self) {
        for account in &self.accounts {
            account.stagger_recovery_if_due();
        }
    }
}

fn snapshot_from_state(state: &QuotaState) -> Option<QuotaSnapshot> {
    match state {
        QuotaState::Ready { snapshot, .. }
        | QuotaState::Stale { snapshot, .. }
        | QuotaState::Cooldown {
            snapshot: Some(snapshot),
            ..
        }
        | QuotaState::Error {
            last_snapshot: Some(snapshot),
            ..
        } => Some(snapshot.clone()),
        _ => None,
    }
}

fn account_jitter(account_id: &str, maximum: Duration) -> Duration {
    if maximum.is_zero() {
        return Duration::ZERO;
    }
    let hash = account_id.bytes().fold(2_166_136_261_u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(16_777_619)
    });
    Duration::from_secs(hash % (maximum.as_secs() + 1))
}

fn chrono_duration(duration: Duration) -> chrono::Duration {
    chrono::Duration::from_std(duration).expect("quota refresh duration is too large")
}

struct UnavailableQuotaSource(String);

impl QuotaSource for UnavailableQuotaSource {
    fn refresh(&self) -> Result<QuotaSnapshot, QuotaRefreshError> {
        Err(QuotaRefreshError::new(
            QuotaFailureKind::Service,
            self.0.clone(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::VecDeque,
        sync::{
            Condvar,
            atomic::{AtomicUsize, Ordering},
        },
        thread,
    };

    use chrono::{TimeZone, Utc};
    use serde_json::json;

    use super::*;
    use crate::quota_app_server::{CodexAppServerSource, normalize_responses};

    struct StaticQuotaSource(QuotaSnapshot);

    impl QuotaSource for StaticQuotaSource {
        fn refresh(&self) -> Result<QuotaSnapshot, QuotaRefreshError> {
            Ok(self.0.clone())
        }
    }

    struct FakeClock(Mutex<DateTime<Utc>>);

    impl FakeClock {
        fn advance(&self, duration: Duration) {
            *self.0.lock().unwrap() += chrono_duration(duration);
        }
    }

    impl QuotaClock for FakeClock {
        fn now(&self) -> DateTime<Utc> {
            *self.0.lock().unwrap()
        }
    }

    struct SequenceSource {
        calls: AtomicUsize,
        results: Mutex<VecDeque<Result<QuotaSnapshot, QuotaRefreshError>>>,
    }

    impl QuotaSource for SequenceSource {
        fn refresh(&self) -> Result<QuotaSnapshot, QuotaRefreshError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.results.lock().unwrap().pop_front().unwrap()
        }
    }

    fn snapshot(percent: u8) -> QuotaSnapshot {
        snapshot_for("account-1", percent)
    }

    fn snapshot_for(account_id: &str, percent: u8) -> QuotaSnapshot {
        QuotaSnapshot {
            account: QuotaAccount {
                id: account_id.into(),
                display_name: "user@example.com".to_string(),
                plan_type: "plus".to_string(),
            },
            windows: vec![QuotaWindow {
                name: "codex · primary".to_string(),
                remaining_percent: percent,
                resets_at: None,
                window_duration_minutes: Some(300),
            }],
            updated_at: Utc.with_ymd_and_hms(2026, 7, 26, 12, 0, 0).unwrap(),
        }
    }

    fn test_service(clock: Arc<FakeClock>, source: Arc<dyn QuotaSource>) -> QuotaService {
        QuotaService::with_dependencies(
            "account-1",
            source,
            None,
            clock,
            RefreshPolicy {
                interval: Duration::from_secs(600),
                jitter: Duration::ZERO,
                manual_cooldown: Duration::from_secs(30),
                recovery_cooldown: Duration::from_secs(60),
                backoff_base: Duration::from_secs(10),
                backoff_max: Duration::from_secs(40),
            },
        )
    }

    #[test]
    fn transport_failure_keeps_the_last_snapshot_stale_instead_of_inventing_zero() {
        let clock = Arc::new(FakeClock(Mutex::new(
            Utc.with_ymd_and_hms(2026, 7, 26, 12, 0, 0).unwrap(),
        )));
        let source = Arc::new(SequenceSource {
            calls: AtomicUsize::new(0),
            results: Mutex::new(VecDeque::from([
                Ok(snapshot(73)),
                Err(QuotaRefreshError::new(
                    QuotaFailureKind::Transport,
                    "offline",
                )),
            ])),
        });
        let service = test_service(clock.clone(), source);

        assert!(matches!(service.manual_refresh(), QuotaState::Ready { .. }));
        clock.advance(Duration::from_secs(30));
        let state = service.manual_refresh();

        assert!(matches!(
            state,
            QuotaState::Stale {
                reason: QuotaErrorReason::Transport,
                ref snapshot,
                ..
            } if snapshot.windows[0].remaining_percent == 73
        ));
    }

    #[test]
    fn manual_refresh_reports_a_per_account_cooldown_without_calling_upstream_again() {
        let clock = Arc::new(FakeClock(Mutex::new(
            Utc.with_ymd_and_hms(2026, 7, 26, 12, 0, 0).unwrap(),
        )));
        let source = Arc::new(SequenceSource {
            calls: AtomicUsize::new(0),
            results: Mutex::new(VecDeque::from([Ok(snapshot(61))])),
        });
        let service = test_service(clock, source.clone());

        service.manual_refresh();
        let state = service.manual_refresh();

        assert_eq!(source.calls.load(Ordering::SeqCst), 1);
        assert!(
            matches!(state, QuotaState::Cooldown { snapshot: Some(_), retry_at } if retry_at == Utc.with_ymd_and_hms(2026, 7, 26, 12, 0, 30).unwrap())
        );
    }

    struct BlockingSource {
        calls: AtomicUsize,
        entered: (Mutex<bool>, Condvar),
        release: (Mutex<bool>, Condvar),
    }

    impl QuotaSource for BlockingSource {
        fn refresh(&self) -> Result<QuotaSnapshot, QuotaRefreshError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            *self.entered.0.lock().unwrap() = true;
            self.entered.1.notify_all();
            let mut released = self.release.0.lock().unwrap();
            while !*released {
                released = self.release.1.wait(released).unwrap();
            }
            Ok(snapshot(55))
        }
    }

    #[test]
    fn concurrent_refreshes_are_single_flight_for_an_account() {
        let clock = Arc::new(FakeClock(Mutex::new(
            Utc.with_ymd_and_hms(2026, 7, 26, 12, 0, 0).unwrap(),
        )));
        let source = Arc::new(BlockingSource {
            calls: AtomicUsize::new(0),
            entered: (Mutex::new(false), Condvar::new()),
            release: (Mutex::new(false), Condvar::new()),
        });
        let service = Arc::new(test_service(clock, source.clone()));
        let worker_service = service.clone();
        let worker = thread::spawn(move || worker_service.manual_refresh());
        let mut entered = source.entered.0.lock().unwrap();
        while !*entered {
            entered = source.entered.1.wait(entered).unwrap();
        }
        drop(entered);

        assert_eq!(service.manual_refresh(), QuotaState::Loading);
        assert_eq!(source.calls.load(Ordering::SeqCst), 1);
        *source.release.0.lock().unwrap() = true;
        source.release.1.notify_all();
        assert!(matches!(worker.join().unwrap(), QuotaState::Ready { .. }));
    }

    #[test]
    fn retries_use_bounded_exponential_backoff() {
        let clock = Arc::new(FakeClock(Mutex::new(
            Utc.with_ymd_and_hms(2026, 7, 26, 12, 0, 0).unwrap(),
        )));
        let failures = (0..4).map(|_| {
            Err(QuotaRefreshError::new(
                QuotaFailureKind::Service,
                "upstream unavailable",
            ))
        });
        let source = Arc::new(SequenceSource {
            calls: AtomicUsize::new(0),
            results: Mutex::new(failures.collect()),
        });
        let service = QuotaService::with_dependencies(
            "account-1",
            source,
            None,
            clock.clone(),
            RefreshPolicy {
                interval: Duration::from_secs(600),
                jitter: Duration::ZERO,
                manual_cooldown: Duration::ZERO,
                recovery_cooldown: Duration::from_secs(60),
                backoff_base: Duration::from_secs(10),
                backoff_max: Duration::from_secs(40),
            },
        );
        let expected_delays = [10, 20, 40, 40];

        for delay in expected_delays {
            let attempted_at = clock.now();
            let state = service.manual_refresh();
            assert!(
                matches!(state, QuotaState::Error { retry_at: Some(retry_at), .. } if retry_at == attempted_at + chrono::Duration::seconds(delay))
            );
            clock.advance(Duration::from_secs(delay as u64));
        }
    }

    #[test]
    fn wake_or_network_recovery_only_refreshes_due_accounts_and_obeys_recovery_cooldown() {
        let clock = Arc::new(FakeClock(Mutex::new(
            Utc.with_ymd_and_hms(2026, 7, 26, 12, 0, 0).unwrap(),
        )));
        let source = Arc::new(SequenceSource {
            calls: AtomicUsize::new(0),
            results: Mutex::new(VecDeque::from([
                Err(QuotaRefreshError::new(
                    QuotaFailureKind::Transport,
                    "offline",
                )),
                Ok(snapshot(49)),
            ])),
        });
        let service = test_service(clock.clone(), source.clone());

        service.recover_if_due();
        assert_eq!(source.calls.load(Ordering::SeqCst), 1);
        clock.advance(Duration::from_secs(10));
        service.recover_if_due();
        assert_eq!(source.calls.load(Ordering::SeqCst), 1);
        clock.advance(Duration::from_secs(50));
        assert!(matches!(service.recover_if_due(), QuotaState::Ready { .. }));
        assert_eq!(source.calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn authentication_failure_requires_reauthorization_and_keeps_the_previous_snapshot() {
        let clock = Arc::new(FakeClock(Mutex::new(
            Utc.with_ymd_and_hms(2026, 7, 26, 12, 0, 0).unwrap(),
        )));
        let source = Arc::new(SequenceSource {
            calls: AtomicUsize::new(0),
            results: Mutex::new(VecDeque::from([
                Ok(snapshot(44)),
                Err(QuotaRefreshError::new(
                    QuotaFailureKind::Authentication,
                    "token expired",
                )),
                Ok(snapshot(88)),
            ])),
        });
        let service = test_service(clock.clone(), source.clone());

        service.manual_refresh();
        clock.advance(Duration::from_secs(30));
        let state = service.manual_refresh();

        assert!(matches!(
            state,
            QuotaState::Error {
                reason: QuotaErrorReason::Reauthorization,
                last_snapshot: Some(ref snapshot),
                retry_at: None,
                ..
            } if snapshot.windows[0].remaining_percent == 44
        ));
        clock.advance(Duration::from_secs(600));
        service.refresh_if_due();
        service.manual_refresh();
        assert_eq!(source.calls.load(Ordering::SeqCst), 2);

        assert!(
            matches!(service.authorization_restored(), QuotaState::Ready { snapshot, .. } if snapshot.windows[0].remaining_percent == 88)
        );
        assert_eq!(source.calls.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn automatic_refreshes_are_staggered_around_ten_minutes_per_account() {
        let start = Utc.with_ymd_and_hms(2026, 7, 26, 12, 0, 0).unwrap();
        let clock = Arc::new(FakeClock(Mutex::new(start)));
        let first = Arc::new(QuotaService::with_dependencies(
            "account-a",
            Arc::new(StaticQuotaSource(snapshot_for("account-a", 80))),
            None,
            clock.clone(),
            RefreshPolicy::default(),
        ));
        let second = Arc::new(QuotaService::with_dependencies(
            "account-b",
            Arc::new(StaticQuotaSource(snapshot_for("account-b", 70))),
            None,
            clock.clone(),
            RefreshPolicy::default(),
        ));
        let coordinator = QuotaRefreshCoordinator::new(vec![first.clone(), second.clone()]);
        clock.advance(Duration::from_secs(120));

        coordinator.refresh_due();

        let first_next = match first.latest() {
            QuotaState::Ready {
                next_refresh_at, ..
            } => next_refresh_at,
            state => panic!("unexpected state: {state:?}"),
        };
        let second_next = match second.latest() {
            QuotaState::Ready {
                next_refresh_at, ..
            } => next_refresh_at,
            state => panic!("unexpected state: {state:?}"),
        };
        let now = clock.now();
        assert!(
            (now + chrono::Duration::minutes(9)..=now + chrono::Duration::minutes(11))
                .contains(&first_next)
        );
        assert!(
            (now + chrono::Duration::minutes(9)..=now + chrono::Duration::minutes(11))
                .contains(&second_next)
        );
        assert_ne!(first_next, second_next);
    }

    #[test]
    fn coordinator_defers_due_accounts_after_wake_instead_of_starting_a_request_storm() {
        let start = Utc.with_ymd_and_hms(2026, 7, 26, 12, 0, 0).unwrap();
        let clock = Arc::new(FakeClock(Mutex::new(start)));
        let first_source = Arc::new(SequenceSource {
            calls: AtomicUsize::new(0),
            results: Mutex::new(VecDeque::from([Ok(snapshot_for("account-a", 80))])),
        });
        let second_source = Arc::new(SequenceSource {
            calls: AtomicUsize::new(0),
            results: Mutex::new(VecDeque::from([Ok(snapshot_for("account-b", 70))])),
        });
        let first = Arc::new(QuotaService::with_dependencies(
            "account-a",
            first_source.clone(),
            None,
            clock.clone(),
            RefreshPolicy::default(),
        ));
        let second = Arc::new(QuotaService::with_dependencies(
            "account-b",
            second_source.clone(),
            None,
            clock.clone(),
            RefreshPolicy::default(),
        ));
        let coordinator = QuotaRefreshCoordinator::new(vec![first, second]);
        clock.advance(Duration::from_secs(120));

        coordinator.stagger_due_recoveries();
        coordinator.refresh_due();

        assert_eq!(first_source.calls.load(Ordering::SeqCst), 0);
        assert_eq!(second_source.calls.load(Ordering::SeqCst), 0);
        clock.advance(Duration::from_secs(121));
        coordinator.refresh_due();
        assert_eq!(first_source.calls.load(Ordering::SeqCst), 1);
        assert_eq!(second_source.calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn normalizes_named_windows_to_remaining_percentage() {
        let account = json!({
            "account": { "type": "chatgpt", "email": "user@example.com", "planType": "plus" },
            "requiresOpenaiAuth": true
        });
        let rate_limits = json!({
            "rateLimits": {},
            "rateLimitsByLimitId": {
                "codex": {
                    "limitId": "codex",
                    "limitName": null,
                    "planType": "plus",
                    "primary": { "usedPercent": 15, "windowDurationMins": 10080, "resetsAt": 1785660345 },
                    "secondary": null
                }
            }
        });
        let observed_at = Utc.with_ymd_and_hms(2026, 7, 26, 12, 0, 0).unwrap();

        let snapshot = normalize_responses(&account, &rate_limits, observed_at).unwrap();

        assert_eq!(snapshot.account.id, AccountId::from("user@example.com"));
        assert_eq!(snapshot.account.display_name, "user@example.com");
        assert_eq!(snapshot.account.plan_type, "plus");
        assert_eq!(snapshot.windows.len(), 1);
        assert_eq!(snapshot.windows[0].name, "codex · primary");
        assert_eq!(snapshot.windows[0].remaining_percent, 85);
        assert_eq!(snapshot.windows[0].window_duration_minutes, Some(10080));
        assert_eq!(snapshot.updated_at, observed_at);
    }

    #[test]
    fn rejects_invalid_percentages_instead_of_inventing_quota() {
        let account = json!({
            "account": { "type": "chatgpt", "email": null, "planType": "plus" }
        });
        let rate_limits = json!({
            "rateLimits": {
                "limitId": "codex",
                "primary": { "usedPercent": 140 }
            },
            "rateLimitsByLimitId": null
        });

        assert!(normalize_responses(&account, &rate_limits, Utc::now()).is_err());
    }

    #[test]
    fn preserves_additional_named_windows_from_future_protocol_versions() {
        let account = json!({
            "account": { "type": "chatgpt", "email": "user@example.com", "planType": "team" }
        });
        let rate_limits = json!({
            "rateLimits": {
                "limitId": "codex",
                "primary": { "usedPercent": 10 },
                "monthly": { "usedPercent": 25, "windowDurationMins": 43200 },
                "metadata": { "future": true }
            }
        });

        let snapshot = normalize_responses(&account, &rate_limits, Utc::now()).unwrap();

        assert_eq!(
            snapshot
                .windows
                .iter()
                .map(|window| window.name.as_str())
                .collect::<Vec<_>>(),
            vec!["codex · primary", "codex · monthly"]
        );
        assert_eq!(snapshot.windows[1].remaining_percent, 75);
    }

    #[test]
    fn persists_a_snapshot_without_restoring_it_to_an_unverified_account() {
        let database = Arc::new(crate::database::Database::in_memory().unwrap());
        let value = snapshot(80);
        let service = QuotaService::with_store(
            "account-1",
            Arc::new(StaticQuotaSource(value.clone())),
            database.clone(),
        );
        assert!(
            matches!(service.manual_refresh(), QuotaState::Ready { snapshot, .. } if snapshot == value)
        );

        let unavailable = QuotaService::unavailable_with_store(
            "account-2",
            "Codex CLI is unavailable".to_string(),
            database,
        );

        assert!(matches!(
            unavailable.latest(),
            QuotaState::Error {
                reason: QuotaErrorReason::Service,
                last_snapshot: None,
                ..
            }
        ));
    }

    #[test]
    fn restored_snapshot_becomes_stale_when_the_first_refresh_after_restart_fails() {
        let database = Arc::new(crate::database::Database::in_memory().unwrap());
        let value = snapshot(67);
        let initial = QuotaService::with_store(
            "account-1",
            Arc::new(StaticQuotaSource(value.clone())),
            database.clone(),
        );
        initial.manual_refresh();
        let failing_source = Arc::new(SequenceSource {
            calls: AtomicUsize::new(0),
            results: Mutex::new(VecDeque::from([Err(QuotaRefreshError::new(
                QuotaFailureKind::Transport,
                "offline after restart",
            ))])),
        });

        let restored = QuotaService::with_store("account-1", failing_source, database);
        assert!(
            matches!(restored.latest(), QuotaState::Ready { ref snapshot, .. } if snapshot == &value)
        );

        assert!(matches!(
            restored.manual_refresh(),
            QuotaState::Stale {
                reason: QuotaErrorReason::Transport,
                ref snapshot,
                ..
            } if snapshot == &value
        ));
    }

    #[test]
    fn current_codex_account_does_not_restore_an_unverified_previous_login_snapshot() {
        let database = Arc::new(crate::database::Database::in_memory().unwrap());
        let mut value = snapshot(62);
        value.account.id = "previous-login".into();
        let initial = QuotaService::with_store(
            CURRENT_CODEX_ACCOUNT_ID,
            Arc::new(StaticQuotaSource(value)),
            database.clone(),
        );
        initial.manual_refresh();

        let restarted = QuotaService::with_store(
            CURRENT_CODEX_ACCOUNT_ID,
            Arc::new(StaticQuotaSource(snapshot(80))),
            database,
        );

        assert_eq!(restarted.latest(), QuotaState::Loading);
    }

    #[test]
    #[ignore = "requires a locally authenticated Codex CLI"]
    fn reads_live_quota_without_exposing_credentials() {
        let snapshot = CodexAppServerSource::discover().unwrap().refresh().unwrap();

        assert!(!snapshot.account.display_name.is_empty());
        assert!(
            snapshot
                .windows
                .iter()
                .all(|window| window.remaining_percent <= 100)
        );
    }
}
