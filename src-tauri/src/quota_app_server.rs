use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::mpsc,
    thread,
    time::Duration,
};

use chrono::Utc;
use serde_json::Value;

use crate::quota::{QuotaSnapshot, QuotaSource, normalize_responses};

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
                    Some(2) => account = Some(result_value(message)?),
                    Some(3) => rate_limits = Some(result_value(message)?),
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
    fn refresh(&self) -> Result<QuotaSnapshot, String> {
        let response = self.request()?;
        normalize_responses(&response.account, &response.rate_limits, Utc::now())
    }
}

struct AppServerQuotaResponse {
    account: Value,
    rate_limits: Value,
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

fn result_value(message: Value) -> Result<Value, String> {
    if let Some(error) = message.get("error") {
        return Err(error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("Codex app-server returned an error")
            .to_string());
    }
    message
        .get("result")
        .cloned()
        .ok_or_else(|| "Codex app-server response did not contain a result".to_string())
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
