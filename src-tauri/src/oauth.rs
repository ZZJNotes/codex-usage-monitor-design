//! Codex OAuth PKCE login flow (Keychain-only — no auth file writes).

use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use base64::Engine;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use url::Url;

pub const AUTH_URL: &str = "https://auth.openai.com/oauth/authorize";
pub const TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
pub const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
pub const REDIRECT_URI: &str = "http://localhost:1455/auth/callback";
pub const OAUTH_SCOPES: &str = "openid email profile offline_access";

pub struct PkceCodes {
    pub code_verifier: String,
    pub code_challenge: String,
}

fn random_string(length: usize) -> String {
    let charset: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-._~";
    (0..length)
        .map(|_| {
            let idx = rand::random::<usize>() % charset.len();
            charset[idx] as char
        })
        .collect()
}

pub fn generate_pkce_codes() -> PkceCodes {
    let code_verifier = random_string(64);
    let hash = Sha256::digest(code_verifier.as_bytes());
    let code_challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(hash);
    PkceCodes {
        code_verifier,
        code_challenge,
    }
}

pub fn generate_random_state() -> String {
    let bytes: Vec<u8> = (0..16).map(|_| rand::random::<u8>()).collect();
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// In-memory token material — never serialized to disk by this module.
#[derive(Debug, Clone)]
pub struct CodexTokenData {
    pub id_token: Option<String>,
    pub access_token: String,
    pub refresh_token: String,
    pub account_id: String,
    pub email: String,
    pub expires_at: String,
}

/// Secret-free OAuth result for IPC.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OAuthResultDto {
    pub account_id: String,
    pub alias: String,
    pub identity_fingerprint: String,
    pub status: String,
}

#[derive(Debug)]
pub enum OAuthError {
    PortInUse,
    CodeExchangeFailed(String),
    CallbackTimeout,
    InvalidState,
    ServerError(String),
    Cancelled,
}

impl std::fmt::Display for OAuthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PortInUse => write!(f, "Port 1455 is already in use"),
            Self::CodeExchangeFailed(msg) => write!(f, "Code exchange failed: {msg}"),
            Self::CallbackTimeout => write!(f, "OAuth callback timed out"),
            Self::InvalidState => write!(f, "State mismatch - possible CSRF"),
            Self::ServerError(msg) => write!(f, "OAuth server error: {msg}"),
            Self::Cancelled => write!(f, "User cancelled the login"),
        }
    }
}

pub fn parse_jwt_payload(token: &str) -> Option<serde_json::Value> {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() < 2 {
        return None;
    }
    let padded = match parts[1].len() % 4 {
        1 => return None,
        2 => format!("{}==", parts[1]),
        3 => format!("{}=", parts[1]),
        _ => parts[1].to_string(),
    };
    let decoded = base64::engine::general_purpose::URL_SAFE
        .decode(&padded)
        .ok()?;
    serde_json::from_slice(&decoded).ok()
}

pub fn extract_account_identity(id_token: &str) -> (String, String) {
    if let Some(payload) = parse_jwt_payload(id_token) {
        let auth = payload.get("https://api.openai.com/auth");
        let account_id = auth
            .and_then(|value| value.get("chatgpt_account_id"))
            .or_else(|| payload.get("https://api.openai.com/auth/user/id"))
            .or_else(|| payload.get("sub"))
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();
        let email = payload
            .get("https://api.openai.com/profile")
            .and_then(|value| value.get("email"))
            .or_else(|| payload.get("email"))
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();
        (account_id, email)
    } else {
        ("unknown".to_string(), "unknown".to_string())
    }
}

pub fn build_authorization_url(pkce: &PkceCodes, state: &str) -> Result<Url, OAuthError> {
    let mut auth_url = Url::parse(AUTH_URL).map_err(|e| OAuthError::ServerError(e.to_string()))?;
    auth_url
        .query_pairs_mut()
        .append_pair("client_id", CLIENT_ID)
        .append_pair("response_type", "code")
        .append_pair("redirect_uri", REDIRECT_URI)
        .append_pair("scope", OAUTH_SCOPES)
        .append_pair("state", state)
        .append_pair("code_challenge", &pkce.code_challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("prompt", "login")
        .append_pair("id_token_add_organizations", "true")
        .append_pair("codex_cli_simplified_flow", "true");
    Ok(auth_url)
}

/// Run PKCE OAuth and return tokens in memory only. Never writes auth.json.
pub fn run_codex_oauth_login(
    cancel: Option<Arc<AtomicBool>>,
) -> Result<CodexTokenData, OAuthError> {
    let pkce = generate_pkce_codes();
    let state = generate_random_state();
    let listener = TcpListener::bind("127.0.0.1:1455").map_err(|_| OAuthError::PortInUse)?;
    let auth_url = build_authorization_url(&pkce, &state)?;
    let _ = std::process::Command::new("open")
        .arg(auth_url.as_str())
        .spawn();
    let (code, received_state) =
        wait_for_oauth_callback(&listener, Duration::from_secs(300), cancel)?;
    if received_state.as_deref() != Some(&state) {
        return Err(OAuthError::InvalidState);
    }
    exchange_code_for_tokens(&code, &pkce.code_verifier)
}

fn exchange_code_for_tokens(code: &str, code_verifier: &str) -> Result<CodexTokenData, OAuthError> {
    let form_body = format!(
        "grant_type=authorization_code&client_id={}&code={}&redirect_uri={}&code_verifier={}",
        urlencode(CLIENT_ID),
        urlencode(code),
        urlencode(REDIRECT_URI),
        urlencode(code_verifier),
    );
    parse_token_response(http_post_form(TOKEN_URL, &form_body)?)
}

pub fn refresh_access_token(refresh_token: &str) -> Result<CodexTokenData, OAuthError> {
    let form_body = format!(
        "client_id={}&grant_type=refresh_token&refresh_token={}&scope=openid+profile+email",
        urlencode(CLIENT_ID),
        urlencode(refresh_token),
    );
    parse_token_response(http_post_form(TOKEN_URL, &form_body)?)
}

fn parse_token_response(response: (u16, String)) -> Result<CodexTokenData, OAuthError> {
    let (status_code, body_bytes) = response;
    if status_code != 200 {
        // Do not echo raw body (may contain secrets); keep status only.
        return Err(OAuthError::CodeExchangeFailed(format!(
            "HTTP {status_code}"
        )));
    }

    #[derive(Deserialize)]
    struct TokenResponse {
        access_token: String,
        refresh_token: String,
        id_token: Option<String>,
        expires_in: i64,
    }

    let token_resp: TokenResponse = serde_json::from_str(&body_bytes)
        .map_err(|_| OAuthError::CodeExchangeFailed("invalid token JSON".into()))?;
    let id_token_str = token_resp.id_token.unwrap_or_default();
    let (account_id, email) = if id_token_str.is_empty() {
        ("unknown".to_string(), "unknown".to_string())
    } else {
        extract_account_identity(&id_token_str)
    };
    let expires_at = (Utc::now() + chrono::Duration::seconds(token_resp.expires_in)).to_rfc3339();
    Ok(CodexTokenData {
        id_token: Some(id_token_str),
        access_token: token_resp.access_token,
        refresh_token: token_resp.refresh_token,
        account_id,
        email,
        expires_at,
    })
}

fn wait_for_oauth_callback(
    listener: &TcpListener,
    timeout: Duration,
    cancel: Option<Arc<AtomicBool>>,
) -> Result<(String, Option<String>), OAuthError> {
    let start = Instant::now();
    listener
        .set_nonblocking(true)
        .map_err(|e| OAuthError::ServerError(e.to_string()))?;

    loop {
        if cancel
            .as_ref()
            .is_some_and(|flag| flag.load(Ordering::SeqCst))
        {
            return Err(OAuthError::Cancelled);
        }
        if start.elapsed() > timeout {
            return Err(OAuthError::CallbackTimeout);
        }
        match listener.accept() {
            Ok((mut stream, _)) => {
                let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
                let mut buf = vec![0u8; 4096];
                match stream.read(&mut buf) {
                    Ok(n) if n > 0 => {
                        let request = String::from_utf8_lossy(&buf[..n]);
                        if let Some((code, state)) = parse_callback_request(&request) {
                            let response = "HTTP/1.1 302 Found\r\nLocation: /success\r\nContent-Length: 0\r\n\r\n";
                            let _ = stream.write_all(response.as_bytes());
                            let _ = stream.flush();
                            return Ok((code, state));
                        } else if request.contains("/success") {
                            let html = "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\n\r\n<!DOCTYPE html><html><body><h1>Authentication successful</h1><p>You may close this window.</p></body></html>";
                            let _ = stream.write_all(html.as_bytes());
                            let _ = stream.flush();
                        }
                    }
                    _ => {}
                }
            }
            Err(ref e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => return Err(OAuthError::ServerError(e.to_string())),
        }
    }
}

fn parse_callback_request(request: &str) -> Option<(String, Option<String>)> {
    let first_line = request.lines().next()?;
    let path = first_line.split_whitespace().nth(1)?;
    if !path.starts_with("/auth/callback") {
        return None;
    }
    let url = Url::parse(&format!("http://localhost{path}")).ok()?;
    let code = url.query_pairs().find(|(k, _)| k == "code")?.1.to_string();
    let state = url
        .query_pairs()
        .find(|(k, _)| k == "state")
        .map(|(_, v)| v.to_string());
    Some((code, state))
}

fn urlencode(input: &str) -> String {
    input
        .chars()
        .map(|c| match c {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' => c.to_string(),
            ' ' => "+".to_string(),
            _ => format!("%{:02X}", c as u8),
        })
        .collect()
}

/// HTTP POST with form body using curl. Response body stays in memory only.
fn http_post_form(url: &str, form_body: &str) -> Result<(u16, String), OAuthError> {
    let output = std::process::Command::new("curl")
        .args([
            "-sS",
            "-w",
            "\n%{http_code}",
            "-X",
            "POST",
            url,
            "-H",
            "Content-Type: application/x-www-form-urlencoded",
            "-H",
            "Accept: application/json",
            "--data-binary",
            form_body,
            "--max-time",
            "30",
        ])
        .output()
        .map_err(|e| OAuthError::CodeExchangeFailed(format!("curl execution: {e}")))?;

    if !output.status.success() && output.stdout.is_empty() {
        return Err(OAuthError::CodeExchangeFailed("curl failed".into()));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut lines: Vec<&str> = stdout.trim_end().rsplitn(2, '\n').collect();
    lines.reverse();
    if lines.len() == 1 {
        // Only status code or only body — treat as failure without leaking.
        let status: u16 = lines[0].parse().unwrap_or(0);
        return Ok((status, String::new()));
    }
    let body = lines[0].to_string();
    let status: u16 = lines[1].parse().unwrap_or(0);
    Ok((status, body))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_jwt_works() {
        let payload = r#"{"sub":"user123","email":"test@example.com"}"#;
        let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload);
        let token = format!("header.{b64}.signature");
        let parsed = parse_jwt_payload(&token).unwrap();
        assert_eq!(parsed["email"], "test@example.com");
    }

    #[test]
    fn generate_pkce_is_valid() {
        let pkce = generate_pkce_codes();
        assert_eq!(pkce.code_verifier.len(), 64);
        let expected = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(Sha256::digest(pkce.code_verifier.as_bytes()));
        assert_eq!(pkce.code_challenge, expected);
    }

    #[test]
    fn generate_state_is_random() {
        assert_ne!(generate_random_state(), generate_random_state());
    }

    #[test]
    fn authorization_url_binds_loopback_and_state() {
        let pkce = generate_pkce_codes();
        let url = build_authorization_url(&pkce, "abc123").unwrap();
        assert!(url.as_str().contains("state=abc123"));
        assert!(url.as_str().contains("code_challenge="));
        assert!(url.as_str().contains("localhost"));
    }

    #[test]
    fn oauth_result_dto_has_no_token_fields() {
        let dto = OAuthResultDto {
            account_id: "local-1".into(),
            alias: "Work".into(),
            identity_fingerprint: "deadbeef".into(),
            status: "active".into(),
        };
        let json = serde_json::to_string(&dto).unwrap();
        assert!(!json.contains("token"));
        assert!(!json.contains("refresh"));
        assert!(!json.contains("access"));
    }
}
