use std::{
    collections::BTreeMap,
    fs,
    io::{BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::mpsc,
    thread,
    time::Duration,
};

use chrono::{DateTime, Utc};
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::Value;

use crate::quota::{
    QuotaAccount, QuotaFailureKind, QuotaRefreshError, QuotaSnapshot, QuotaSource, QuotaWindow,
};

pub struct CodexAppServerSource {
    executable: PathBuf,
}

impl CodexAppServerSource {
    pub fn discover() -> Result<Self, String> {
        resolve_codex_executable()
            .map(|executable| Self { executable })
            .ok_or_else(|| "Codex CLI is unavailable".to_string())
    }

    fn request(&self) -> Result<AppServerQuotaResponse, String> {
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
            .map_err(|error| format!("Could not start Codex app-server: {error}"))?;
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| "Could not connect to Codex app-server input".to_string())?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "Could not connect to Codex app-server output".to_string())?;
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
                    .map_err(|_| "Reading ChatGPT quota timed out".to_string())?;
                match message.get("id").and_then(Value::as_i64) {
                    Some(2) => account = Some(result_value::<GetAccountResponse>(message)?),
                    Some(3) => {
                        rate_limits = Some(result_value::<GetAccountRateLimitsResponse>(message)?)
                    }
                    _ => {}
                }
            }
            Ok(AppServerQuotaResponse {
                account: account.unwrap(),
                rate_limits: rate_limits.unwrap(),
            })
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
                Err(format!("{message}: {}", stderr_text.trim()))
            }
            result => result,
        }
    }
}

impl QuotaSource for CodexAppServerSource {
    fn refresh(&self) -> Result<QuotaSnapshot, QuotaRefreshError> {
        let response = self.request().map_err(classify_app_server_error)?;
        normalize_response(response, Utc::now())
            .map_err(|message| QuotaRefreshError::new(QuotaFailureKind::InvalidResponse, message))
    }
}

struct AppServerQuotaResponse {
    account: GetAccountResponse,
    rate_limits: GetAccountRateLimitsResponse,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GetAccountResponse {
    account: Option<Account>,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
enum Account {
    Chatgpt {
        email: Option<String>,
        #[serde(rename = "planType")]
        plan_type: String,
    },
    #[serde(other)]
    Unsupported,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GetAccountRateLimitsResponse {
    rate_limits: RateLimitSnapshot,
    rate_limits_by_limit_id: Option<BTreeMap<String, RateLimitSnapshot>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RateLimitSnapshot {
    limit_id: Option<String>,
    limit_name: Option<String>,
    #[serde(flatten)]
    fields: BTreeMap<String, Value>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RateLimitWindow {
    used_percent: i64,
    resets_at: Option<i64>,
    window_duration_mins: Option<u64>,
}

fn normalize_response(
    response: AppServerQuotaResponse,
    observed_at: DateTime<Utc>,
) -> Result<QuotaSnapshot, String> {
    let Account::Chatgpt { email, plan_type } = response
        .account
        .account
        .ok_or_else(|| "The current Codex login is not a ChatGPT account".to_string())?
    else {
        return Err("The current Codex login is not a ChatGPT account".to_string());
    };
    let display_name = email.unwrap_or_else(|| "ChatGPT account".to_string());
    let mut buckets = response
        .rate_limits
        .rate_limits_by_limit_id
        .filter(|buckets| !buckets.is_empty())
        .unwrap_or_else(|| {
            let snapshot = response.rate_limits.rate_limits;
            BTreeMap::from([(
                snapshot
                    .limit_id
                    .clone()
                    .unwrap_or_else(|| "codex".to_string()),
                snapshot,
            )])
        });
    let mut windows = Vec::new();
    for (bucket_key, snapshot) in &mut buckets {
        let bucket_name = snapshot
            .limit_name
            .as_deref()
            .or(snapshot.limit_id.as_deref())
            .unwrap_or(bucket_key);
        let mut named_windows = snapshot
            .fields
            .iter()
            .filter(|(_, value)| value.get("usedPercent").is_some())
            .map(|(name, value)| {
                serde_json::from_value::<RateLimitWindow>(value.clone())
                    .map(|window| (name, window))
                    .map_err(|error| {
                        format!(
                            "{bucket_name} {name} did not match the quota window schema: {error}"
                        )
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;
        named_windows.sort_by_key(|(name, _)| match name.as_str() {
            "primary" => (0, name.as_str()),
            "secondary" => (1, name.as_str()),
            _ => (2, name.as_str()),
        });
        for (window_name, window) in named_windows {
            if !(0..=100).contains(&window.used_percent) {
                return Err(format!(
                    "{bucket_name} {window_name} has an invalid quota percentage"
                ));
            }
            let resets_at = window
                .resets_at
                .map(|timestamp| {
                    DateTime::from_timestamp(timestamp, 0).ok_or_else(|| {
                        format!("{bucket_name} {window_name} has an invalid reset time")
                    })
                })
                .transpose()?;
            windows.push(QuotaWindow {
                name: format!("{bucket_name} · {window_name}"),
                remaining_percent: (100 - window.used_percent) as u8,
                resets_at,
                window_duration_minutes: window.window_duration_mins,
            });
        }
    }
    Ok(QuotaSnapshot {
        account: QuotaAccount {
            id: display_name.clone(),
            display_name,
            plan_type,
        },
        windows,
        updated_at: observed_at,
    })
}

fn classify_app_server_error(message: String) -> QuotaRefreshError {
    let lowercase = message.to_ascii_lowercase();
    let kind = if lowercase.contains("did not match its schema")
        || lowercase.contains("did not contain a result")
    {
        QuotaFailureKind::InvalidResponse
    } else if lowercase.contains("auth")
        || lowercase.contains("login")
        || lowercase.contains("unauthorized")
        || lowercase.contains("forbidden")
        || lowercase.contains("401")
        || lowercase.contains("403")
    {
        QuotaFailureKind::Authentication
    } else if lowercase.contains("timed out")
        || lowercase.contains("could not start")
        || lowercase.contains("connect")
        || lowercase.contains("offline")
    {
        QuotaFailureKind::Transport
    } else {
        QuotaFailureKind::Service
    };
    QuotaRefreshError::new(kind, message)
}

#[cfg(test)]
pub(crate) fn normalize_responses(
    account: &Value,
    rate_limits: &Value,
    observed_at: DateTime<Utc>,
) -> Result<QuotaSnapshot, String> {
    normalize_response(
        AppServerQuotaResponse {
            account: serde_json::from_value(account.clone()).map_err(|error| error.to_string())?,
            rate_limits: serde_json::from_value(rate_limits.clone())
                .map_err(|error| error.to_string())?,
        },
        observed_at,
    )
}

fn send_request(stdin: &mut impl Write, request: Value) -> Result<(), String> {
    writeln!(stdin, "{request}").map_err(|error| error.to_string())?;
    stdin.flush().map_err(|error| error.to_string())
}

fn wait_for_result(receiver: &mpsc::Receiver<Value>, id: i64) -> Result<Value, String> {
    loop {
        let message = receiver
            .recv_timeout(Duration::from_secs(10))
            .map_err(|_| "Initializing Codex app-server timed out".to_string())?;
        if message.get("id").and_then(Value::as_i64) == Some(id) {
            return result_value(message);
        }
    }
}

fn result_value<T: DeserializeOwned>(message: Value) -> Result<T, String> {
    if let Some(error) = message.get("error") {
        return Err(error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("Codex app-server returned an error")
            .to_string());
    }
    let result = message
        .get("result")
        .cloned()
        .ok_or_else(|| "Codex app-server response did not contain a result".to_string())?;
    serde_json::from_value(result)
        .map_err(|error| format!("Codex app-server response did not match its schema: {error}"))
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
