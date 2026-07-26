use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, Mutex, RwLock, mpsc},
    thread,
    time::Duration,
};

use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaAccount {
    pub display_name: String,
    pub plan_type: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaWindow {
    pub name: String,
    pub remaining_percent: u8,
    pub resets_at: Option<DateTime<Utc>>,
    pub window_duration_minutes: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
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
    },
    Error {
        message: String,
        last_snapshot: Option<QuotaSnapshot>,
    },
}

pub trait QuotaSource: Send + Sync {
    fn refresh(&self) -> Result<QuotaSnapshot, String>;
}

pub struct QuotaService {
    source: Arc<dyn QuotaSource>,
    state: RwLock<QuotaState>,
    refresh_lock: Mutex<()>,
}

impl QuotaService {
    pub fn new(source: Arc<dyn QuotaSource>) -> Self {
        Self {
            source,
            state: RwLock::new(QuotaState::Loading),
            refresh_lock: Mutex::new(()),
        }
    }

    pub fn unavailable(message: String) -> Self {
        let service = Self::new(Arc::new(UnavailableQuotaSource(message.clone())));
        *service.state.write().expect("quota state poisoned") = QuotaState::Error {
            message,
            last_snapshot: None,
        };
        service
    }

    pub fn latest(&self) -> QuotaState {
        self.state.read().expect("quota state poisoned").clone()
    }

    pub fn refresh(&self) -> QuotaState {
        let _refresh = self
            .refresh_lock
            .lock()
            .expect("quota refresh lock poisoned");
        match self.source.refresh() {
            Ok(snapshot) => {
                let state = QuotaState::Ready { snapshot };
                *self.state.write().expect("quota state poisoned") = state.clone();
                state
            }
            Err(message) => {
                let last_snapshot = match self.latest() {
                    QuotaState::Ready { snapshot }
                    | QuotaState::Error {
                        last_snapshot: Some(snapshot),
                        ..
                    } => Some(snapshot),
                    _ => None,
                };
                let state = QuotaState::Error {
                    message,
                    last_snapshot,
                };
                *self.state.write().expect("quota state poisoned") = state.clone();
                state
            }
        }
    }
}

struct UnavailableQuotaSource(String);

impl QuotaSource for UnavailableQuotaSource {
    fn refresh(&self) -> Result<QuotaSnapshot, String> {
        Err(self.0.clone())
    }
}

pub struct CodexAppServerSource {
    executable: PathBuf,
}

impl CodexAppServerSource {
    pub fn discover() -> Result<Self, String> {
        resolve_codex_executable()
            .map(|executable| Self { executable })
            .ok_or_else(|| "找不到 Codex CLI；请确认已安装并可从终端运行 codex".to_string())
    }

    fn request(&self) -> Result<(Value, Value), String> {
        let mut command = Command::new(&self.executable);
        command.args(["app-server", "--stdio"]);
        if let Some(directory) = self.executable.parent() {
            let mut paths = vec![directory.to_path_buf()];
            if let Some(existing) = std::env::var_os("PATH") {
                paths.extend(std::env::split_paths(&existing));
            }
            if let Ok(path) = std::env::join_paths(paths) {
                command.env("PATH", path);
            }
        }
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| format!("无法启动 Codex app-server：{error}"))?;
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| "无法连接 Codex app-server 输入".to_string())?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "无法连接 Codex app-server 输出".to_string())?;
        let mut stderr = child.stderr.take();
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if let Ok(value) = serde_json::from_str::<Value>(&line) {
                    let _ = sender.send(value);
                }
            }
        });

        let result = (|| {
            send_request(
                &mut stdin,
                serde_json::json!({
                    "id": 1,
                    "method": "initialize",
                    "params": {
                        "clientInfo": { "name": "codex-usage-monitor", "version": env!("CARGO_PKG_VERSION") },
                        "capabilities": { "experimentalApi": true }
                    }
                }),
            )?;
            wait_for_result(&receiver, 1)?;
            send_request(
                &mut stdin,
                serde_json::json!({ "method": "initialized", "params": {} }),
            )?;
            send_request(
                &mut stdin,
                serde_json::json!({ "id": 2, "method": "account/read", "params": { "refreshToken": false } }),
            )?;
            send_request(
                &mut stdin,
                serde_json::json!({ "id": 3, "method": "account/rateLimits/read" }),
            )?;
            let mut account = None;
            let mut rate_limits = None;
            while account.is_none() || rate_limits.is_none() {
                let message = receiver
                    .recv_timeout(Duration::from_secs(15))
                    .map_err(|_| "读取 ChatGPT 额度超时".to_string())?;
                match message.get("id").and_then(Value::as_i64) {
                    Some(2) => account = Some(result_value(message)?),
                    Some(3) => rate_limits = Some(result_value(message)?),
                    _ => {}
                }
            }
            Ok((account.unwrap(), rate_limits.unwrap()))
        })();
        drop(stdin);
        let _ = child.kill();
        let _ = child.wait();
        let mut stderr_text = String::new();
        if let Some(stderr) = stderr.as_mut() {
            let _ = stderr.read_to_string(&mut stderr_text);
        }
        match result {
            Err(message) if !stderr_text.trim().is_empty() => {
                Err(format!("{message}：{}", stderr_text.trim()))
            }
            result => result,
        }
    }
}

impl QuotaSource for CodexAppServerSource {
    fn refresh(&self) -> Result<QuotaSnapshot, String> {
        let (account, rate_limits) = self.request()?;
        normalize_responses(&account, &rate_limits, Utc::now())
    }
}

fn send_request(stdin: &mut impl Write, request: Value) -> Result<(), String> {
    writeln!(stdin, "{request}").map_err(|error| error.to_string())?;
    stdin.flush().map_err(|error| error.to_string())
}

fn wait_for_result(receiver: &mpsc::Receiver<Value>, id: i64) -> Result<Value, String> {
    loop {
        let message = receiver
            .recv_timeout(Duration::from_secs(10))
            .map_err(|_| "初始化 Codex app-server 超时".to_string())?;
        if message.get("id").and_then(Value::as_i64) == Some(id) {
            return result_value(message);
        }
    }
}

fn result_value(message: Value) -> Result<Value, String> {
    if let Some(error) = message.get("error") {
        return Err(error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("Codex app-server 返回错误")
            .to_string());
    }
    message
        .get("result")
        .cloned()
        .ok_or_else(|| "Codex app-server 响应缺少 result".to_string())
}

fn resolve_codex_executable() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("CODEX_USAGE_MONITOR_CODEX_PATH") {
        let path = PathBuf::from(path);
        if is_executable_file(&path) {
            return Some(path);
        }
    }
    if let Some(path) = std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|directory| directory.join("codex"))
            .find(|path| is_executable_file(path))
    }) {
        return Some(path);
    }
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    let versions = home.join(".nvm/versions/node");
    let mut candidates = fs::read_dir(versions)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|entry| entry.path().join("bin/codex"))
        .filter(|path| is_executable_file(path))
        .collect::<Vec<_>>();
    candidates.sort();
    if let Some(path) = candidates.pop() {
        return Some(path);
    }
    [
        PathBuf::from("/opt/homebrew/bin/codex"),
        PathBuf::from("/usr/local/bin/codex"),
    ]
    .into_iter()
    .find(|path| is_executable_file(path))
}

fn is_executable_file(path: &Path) -> bool {
    path.is_file()
}

fn normalize_responses(
    account_result: &Value,
    rate_limit_result: &Value,
    observed_at: DateTime<Utc>,
) -> Result<QuotaSnapshot, String> {
    let account = account_result
        .get("account")
        .filter(|account| account.get("type").and_then(Value::as_str) == Some("chatgpt"))
        .ok_or_else(|| "当前 Codex 未使用 ChatGPT 账户登录".to_string())?;
    let display_name = account
        .get("email")
        .and_then(Value::as_str)
        .unwrap_or("ChatGPT account")
        .to_string();
    let plan_type = account
        .get("planType")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_string();
    let mut buckets = Vec::new();
    if let Some(by_limit_id) = rate_limit_result
        .get("rateLimitsByLimitId")
        .and_then(Value::as_object)
        .filter(|value| !value.is_empty())
    {
        let mut entries = by_limit_id.iter().collect::<Vec<_>>();
        entries.sort_by_key(|(key, _)| *key);
        for (key, snapshot) in entries {
            buckets.push((key.as_str(), snapshot));
        }
    } else if let Some(snapshot) = rate_limit_result.get("rateLimits") {
        let key = snapshot
            .get("limitId")
            .and_then(Value::as_str)
            .unwrap_or("codex");
        buckets.push((key, snapshot));
    }
    let mut windows = Vec::new();
    for (bucket_key, snapshot) in buckets {
        let bucket_name = snapshot
            .get("limitName")
            .and_then(Value::as_str)
            .or_else(|| snapshot.get("limitId").and_then(Value::as_str))
            .unwrap_or(bucket_key);
        for window_name in ["primary", "secondary"] {
            let Some(window) = snapshot.get(window_name).filter(|value| !value.is_null()) else {
                continue;
            };
            let used_percent = window
                .get("usedPercent")
                .and_then(Value::as_i64)
                .filter(|value| (0..=100).contains(value))
                .ok_or_else(|| format!("{bucket_name} {window_name} 的额度百分比无效"))?;
            let resets_at = window
                .get("resetsAt")
                .and_then(Value::as_i64)
                .map(|timestamp| {
                    DateTime::from_timestamp(timestamp, 0)
                        .ok_or_else(|| format!("{bucket_name} {window_name} 的重置时间无效"))
                })
                .transpose()?;
            windows.push(QuotaWindow {
                name: format!("{bucket_name} · {window_name}"),
                remaining_percent: (100 - used_percent) as u8,
                resets_at,
                window_duration_minutes: window.get("windowDurationMins").and_then(Value::as_u64),
            });
        }
    }
    Ok(QuotaSnapshot {
        account: QuotaAccount {
            display_name,
            plan_type,
        },
        windows,
        updated_at: observed_at,
    })
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};
    use serde_json::json;

    use super::*;

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
