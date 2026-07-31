//! Direct HTTPS quota source for managed accounts (Keychain-only).
//!
//! Uses the approved ChatGPT usage endpoint with an in-memory access token.
//! Never writes auth.json or temporary credential files.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::Deserialize;

use crate::{
    credentials::{CredentialService, CredentialStoreError},
    oauth::{self, OAuthError},
    quota::{
        AccountId, QuotaAccount, QuotaFailureKind, QuotaRefreshError, QuotaSnapshot, QuotaSource,
        QuotaWindow,
    },
};

pub const USAGE_URL: &str = "https://chatgpt.com/backend-api/wham/usage";

/// Managed-account quota source: refresh token from Keychain → memory access token → HTTPS usage.
pub struct DirectHttpsQuotaSource {
    account_id: AccountId,
    expected_fingerprint: String,
    display_name: String,
    credentials: Arc<CredentialService>,
}

impl DirectHttpsQuotaSource {
    pub fn new(
        account_id: impl Into<AccountId>,
        expected_fingerprint: impl Into<String>,
        display_name: impl Into<String>,
        credentials: Arc<CredentialService>,
    ) -> Self {
        Self {
            account_id: account_id.into(),
            expected_fingerprint: expected_fingerprint.into(),
            display_name: display_name.into(),
            credentials,
        }
    }

    fn access_token_for_request(&self) -> Result<(String, String, u64), QuotaRefreshError> {
        let envelope = self
            .credentials
            .load_envelope(self.account_id.as_str())
            .map_err(|error| match error {
                CredentialStoreError::NotFound => QuotaRefreshError::new(
                    QuotaFailureKind::Authentication,
                    "managed credential missing",
                ),
                CredentialStoreError::Locked => {
                    QuotaRefreshError::new(QuotaFailureKind::Service, "keychain locked or denied")
                }
                other => QuotaRefreshError::new(QuotaFailureKind::Service, other.to_string()),
            })?;

        let tokens =
            oauth::refresh_access_token(&envelope.refresh_token).map_err(|error| match error {
                OAuthError::CodeExchangeFailed(message)
                    if message.contains("401") || message.contains("403") =>
                {
                    let _ = self
                        .credentials
                        .mark_reauthorization_required(self.account_id.as_str());
                    QuotaRefreshError::new(QuotaFailureKind::Authentication, "refresh rejected")
                }
                other => QuotaRefreshError::new(QuotaFailureKind::Transport, other.to_string()),
            })?;

        // Persist rotated refresh token with CAS when upstream returns a new one.
        if tokens.refresh_token != envelope.refresh_token {
            match self.credentials.rotate_refresh_token(
                self.account_id.as_str(),
                envelope.generation,
                &tokens.refresh_token,
            ) {
                Ok(_) => {}
                Err(_) => {
                    let _ = self
                        .credentials
                        .mark_reauthorization_required(self.account_id.as_str());
                    return Err(QuotaRefreshError::new(
                        QuotaFailureKind::Authentication,
                        "refresh token rotation could not be persisted",
                    ));
                }
            }
        }

        Ok((tokens.access_token, tokens.account_id, envelope.generation))
    }
}

impl QuotaSource for DirectHttpsQuotaSource {
    fn refresh(&self) -> Result<QuotaSnapshot, QuotaRefreshError> {
        let (access_token, openai_account_id, _generation) = self.access_token_for_request()?;
        let fingerprint = crate::credentials::identity_fingerprint(&openai_account_id);
        if !self.expected_fingerprint.is_empty() && fingerprint != self.expected_fingerprint {
            return Err(QuotaRefreshError::new(
                QuotaFailureKind::Authentication,
                "quota response account fingerprint mismatch",
            ));
        }

        let (status, body) = http_get_usage(&access_token, &openai_account_id)?;
        match status {
            200 => normalize_wham_usage(
                &body,
                self.account_id.as_str(),
                &self.display_name,
                Utc::now(),
            ),
            401 | 403 => {
                let _ = self
                    .credentials
                    .mark_reauthorization_required(self.account_id.as_str());
                Err(QuotaRefreshError::new(
                    QuotaFailureKind::Authentication,
                    "usage endpoint rejected credentials",
                ))
            }
            429 => Err(QuotaRefreshError::new(
                QuotaFailureKind::Transport,
                "usage endpoint rate limited",
            )),
            500..=599 => Err(QuotaRefreshError::new(
                QuotaFailureKind::Service,
                format!("usage endpoint HTTP {status}"),
            )),
            _ => Err(QuotaRefreshError::new(
                QuotaFailureKind::InvalidResponse,
                format!("usage endpoint HTTP {status}"),
            )),
        }
    }
}

fn http_get_usage(
    access_token: &str,
    chatgpt_account_id: &str,
) -> Result<(u16, String), QuotaRefreshError> {
    let auth_header = format!("Authorization: Bearer {access_token}");
    let account_header = format!("ChatGPT-Account-Id: {chatgpt_account_id}");
    let output = std::process::Command::new("curl")
        .args([
            "-sS",
            "-w",
            "\n%{http_code}",
            "-X",
            "GET",
            USAGE_URL,
            "-H",
            &auth_header,
            "-H",
            &account_header,
            "-H",
            "Accept: application/json",
            "-H",
            "User-Agent: codex-cli",
            "--max-time",
            "30",
        ])
        .output()
        .map_err(|error| {
            QuotaRefreshError::new(
                QuotaFailureKind::Transport,
                format!("usage request failed: {error}"),
            )
        })?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut parts: Vec<&str> = stdout.trim_end().rsplitn(2, '\n').collect();
    parts.reverse();
    if parts.is_empty() {
        return Err(QuotaRefreshError::new(
            QuotaFailureKind::Transport,
            "empty usage response",
        ));
    }
    if parts.len() == 1 {
        let status: u16 = parts[0].parse().unwrap_or(0);
        return Ok((status, String::new()));
    }
    let body = parts[0].to_string();
    let status: u16 = parts[1].parse().unwrap_or(0);
    Ok((status, body))
}

#[derive(Debug, Deserialize)]
struct WhamUsageResponse {
    plan_type: Option<String>,
    rate_limit: Option<WhamRateLimit>,
    #[serde(default)]
    code_review_rate_limit: Option<WhamRateLimit>,
}

#[derive(Debug, Deserialize)]
struct WhamRateLimit {
    primary_window: Option<WhamWindow>,
    secondary_window: Option<WhamWindow>,
}

#[derive(Debug, Deserialize)]
struct WhamWindow {
    used_percent: f64,
    reset_at: Option<i64>,
    limit_window_seconds: Option<u64>,
}

pub(crate) fn normalize_wham_usage(
    body: &str,
    local_account_id: &str,
    display_name: &str,
    observed_at: DateTime<Utc>,
) -> Result<QuotaSnapshot, QuotaRefreshError> {
    let parsed: WhamUsageResponse = serde_json::from_str(body).map_err(|_| {
        QuotaRefreshError::new(
            QuotaFailureKind::InvalidResponse,
            "usage response schema mismatch",
        )
    })?;
    let plan_type = parsed.plan_type.unwrap_or_else(|| "unknown".to_string());
    let mut windows = Vec::new();
    if let Some(rate_limit) = parsed.rate_limit {
        push_window(&mut windows, "codex · primary", rate_limit.primary_window)?;
        push_window(
            &mut windows,
            "codex · secondary",
            rate_limit.secondary_window,
        )?;
    }
    if let Some(rate_limit) = parsed.code_review_rate_limit {
        push_window(
            &mut windows,
            "code review · primary",
            rate_limit.primary_window,
        )?;
        push_window(
            &mut windows,
            "code review · secondary",
            rate_limit.secondary_window,
        )?;
    }
    Ok(QuotaSnapshot {
        account: QuotaAccount {
            id: local_account_id.into(),
            display_name: display_name.to_string(),
            plan_type,
        },
        windows,
        updated_at: observed_at,
    })
}

fn push_window(
    windows: &mut Vec<QuotaWindow>,
    name: &str,
    window: Option<WhamWindow>,
) -> Result<(), QuotaRefreshError> {
    let Some(window) = window else {
        return Ok(());
    };
    let used = window.used_percent.round() as i64;
    if !(0..=100).contains(&used) {
        return Err(QuotaRefreshError::new(
            QuotaFailureKind::InvalidResponse,
            format!("{name} has an invalid quota percentage"),
        ));
    }
    let resets_at = window
        .reset_at
        .map(|timestamp| {
            DateTime::from_timestamp(timestamp, 0).ok_or_else(|| {
                QuotaRefreshError::new(
                    QuotaFailureKind::InvalidResponse,
                    format!("{name} has an invalid reset time"),
                )
            })
        })
        .transpose()?;
    windows.push(QuotaWindow {
        name: name.to_string(),
        remaining_percent: (100 - used) as u8,
        resets_at,
        window_duration_minutes: window.limit_window_seconds.map(|seconds| seconds / 60),
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_wham_usage_windows() {
        let body = r#"{
            "plan_type": "plus",
            "rate_limit": {
                "primary_window": {
                    "used_percent": 15,
                    "reset_at": 1785660345,
                    "limit_window_seconds": 18000
                },
                "secondary_window": {
                    "used_percent": 5,
                    "reset_at": 1786000000,
                    "limit_window_seconds": 604800
                }
            }
        }"#;
        let snapshot = normalize_wham_usage(body, "local-1", "Work", Utc::now()).unwrap();
        assert_eq!(snapshot.account.id, AccountId::from("local-1"));
        assert_eq!(snapshot.account.plan_type, "plus");
        assert_eq!(snapshot.windows.len(), 2);
        assert_eq!(snapshot.windows[0].remaining_percent, 85);
        assert_eq!(snapshot.windows[0].window_duration_minutes, Some(300));
        assert_eq!(snapshot.windows[1].remaining_percent, 95);
    }

    #[test]
    fn rejects_invalid_percentages() {
        let body = r#"{
            "plan_type": "plus",
            "rate_limit": {
                "primary_window": { "used_percent": 140 }
            }
        }"#;
        assert!(normalize_wham_usage(body, "local-1", "Work", Utc::now()).is_err());
    }
}
