use std::collections::BTreeMap;

use chrono::Utc;
use rusqlite::{params, params_from_iter, types::Value as SqlValue};

use crate::database::Database;

use super::{
    ActiveAccountEvidence, AttributionSource, ModelUsage, SessionAttribution, SessionUsage,
    TokenAccount, TokenCounts, TokenEvent, TokenUsageData, TokenUsageFilters,
    UNASSIGNED_ACCOUNT_FILTER, hex_digest,
};

pub(super) fn record_account_evidence(
    database: &Database,
    evidence: &ActiveAccountEvidence,
) -> Result<(), String> {
    database.with_connection(|connection| {
        connection
            .execute(
                "INSERT INTO token_accounts
                   (account_key, display_name, last_evidence_source, last_evidence_at_utc)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(account_key) DO UPDATE SET
                   display_name = excluded.display_name,
                   last_evidence_source = excluded.last_evidence_source,
                   last_evidence_at_utc = excluded.last_evidence_at_utc",
                params![
                    evidence.account.account_key,
                    evidence.account.display_name,
                    evidence.source,
                    evidence.observed_at,
                ],
            )
            .map(|_| ())
            .map_err(|error| error.to_string())
    })
}

pub(super) fn record_session_attribution(
    database: &Database,
    session_id: &str,
    evidence: Option<&ActiveAccountEvidence>,
) -> Result<(), String> {
    database.with_connection(|connection| {
        let transaction = connection
            .transaction()
            .map_err(|error| error.to_string())?;
        if let Some(evidence) = evidence {
            transaction
                .execute(
                    "INSERT INTO token_accounts
                       (account_key, display_name, last_evidence_source, last_evidence_at_utc)
                     VALUES (?1, ?2, ?3, ?4)
                     ON CONFLICT(account_key) DO UPDATE SET
                       display_name = excluded.display_name,
                       last_evidence_source = excluded.last_evidence_source,
                       last_evidence_at_utc = excluded.last_evidence_at_utc",
                    params![
                        evidence.account.account_key,
                        evidence.account.display_name,
                        evidence.source,
                        evidence.observed_at,
                    ],
                )
                .map_err(|error| error.to_string())?;
        }
        let assigned_at = Utc::now().to_rfc3339();
        transaction
            .execute(
                "INSERT OR IGNORE INTO token_session_attributions
                   (session_id, account_key, attribution_source, assigned_at_utc,
                    evidence_source, evidence_observed_at_utc)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    session_id,
                    evidence.map(|value| value.account.account_key.as_str()),
                    if evidence.is_some() {
                        "activeAccount"
                    } else {
                        "unassigned"
                    },
                    assigned_at,
                    evidence.map(|value| value.source.as_str()),
                    evidence.map(|value| value.observed_at.as_str()),
                ],
            )
            .map_err(|error| error.to_string())?;
        transaction.commit().map_err(|error| error.to_string())
    })
}

pub(super) fn reassign_session(
    database: &Database,
    session_id: &str,
    account_key: Option<&str>,
    assigned_at: &str,
) -> Result<(), String> {
    database.with_connection(|connection| {
        if let Some(account_key) = account_key {
            let known = connection
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM token_accounts WHERE account_key = ?1)",
                    [account_key],
                    |row| row.get::<_, bool>(0),
                )
                .map_err(|error| error.to_string())?;
            if !known {
                return Err("account is not known to the token usage service".to_string());
            }
        }
        let changed = connection
            .execute(
                "UPDATE token_session_attributions SET
                   account_key = ?1, attribution_source = 'manual', assigned_at_utc = ?2,
                   evidence_source = NULL, evidence_observed_at_utc = NULL
                 WHERE session_id = ?3",
                params![account_key, assigned_at, session_id],
            )
            .map_err(|error| error.to_string())?;
        if changed == 0 {
            Err("session is not known to the token usage service".to_string())
        } else {
            Ok(())
        }
    })
}

pub(super) fn insert_event(
    database: &Database,
    source_key: &str,
    event: &TokenEvent,
) -> Result<bool, String> {
    database.with_connection(|connection| {
        connection
            .execute(
                "INSERT OR IGNORE INTO token_usage_events
                   (event_key, source_key, observed_at_utc, session_id, model,
                    input_tokens, cached_input_tokens, cache_write_input_tokens,
                    output_tokens, reasoning_output_tokens, total_tokens)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                params![
                    event_key(event),
                    source_key,
                    event.observed_at,
                    event.session_id,
                    event.model,
                    to_i64(event.counts.input_tokens),
                    to_i64(event.counts.cached_input_tokens),
                    to_i64(event.counts.cache_write_input_tokens),
                    to_i64(event.counts.output_tokens),
                    to_i64(event.counts.reasoning_output_tokens),
                    to_i64(event.counts.total_tokens),
                ],
            )
            .map(|changed| changed == 1)
            .map_err(|error| error.to_string())
    })
}

fn event_key(event: &TokenEvent) -> String {
    hex_digest(
        format!(
            "{}|{}|{}|{}|{}|{}|{}|{}|{}",
            event.session_id,
            event.observed_at,
            event.model,
            event.event_ordinal,
            event.counts.input_tokens,
            event.counts.cached_input_tokens,
            event.counts.cache_write_input_tokens,
            event.counts.output_tokens,
            event.counts.reasoning_output_tokens,
        )
        .as_bytes(),
    )
}

fn to_i64(value: u64) -> i64 {
    value.min(i64::MAX as u64) as i64
}

pub(super) fn query_usage(
    database: &Database,
    filters: &TokenUsageFilters,
) -> Result<TokenUsageData, String> {
    database.with_connection(|connection| {
        let mut sql = String::from(
            "SELECT observed_at_utc, session_id, model, input_tokens,
                    cached_input_tokens, cache_write_input_tokens, output_tokens,
                    reasoning_output_tokens, total_tokens
             FROM token_usage_events WHERE 1 = 1",
        );
        let mut values = Vec::<SqlValue>::new();
        for (column, value, operator) in [
            ("observed_at_utc", filters.start_at.as_ref(), ">="),
            ("observed_at_utc", filters.end_at.as_ref(), "<="),
            ("model", filters.model.as_ref(), "="),
            ("session_id", filters.session_id.as_ref(), "="),
        ] {
            if let Some(value) = value.filter(|value| !value.is_empty()) {
                sql.push_str(&format!(" AND {column} {operator} ?"));
                values.push(SqlValue::Text(value.clone()));
            }
        }
        if let Some(account_key) = filters.account_key.as_deref().filter(|value| !value.is_empty()) {
            if account_key == UNASSIGNED_ACCOUNT_FILTER {
                sql.push_str(" AND EXISTS (SELECT 1 FROM token_session_attributions attribution WHERE attribution.session_id = token_usage_events.session_id AND attribution.account_key IS NULL)");
            } else {
                sql.push_str(" AND EXISTS (SELECT 1 FROM token_session_attributions attribution WHERE attribution.session_id = token_usage_events.session_id AND attribution.account_key = ?)");
                values.push(SqlValue::Text(account_key.to_string()));
            }
        }
        sql.push_str(" ORDER BY observed_at_utc, event_key");
        let mut statement = connection
            .prepare(&sql)
            .map_err(|error| error.to_string())?;
        let rows = statement
            .query_map(params_from_iter(values), |row| {
                Ok(TokenEvent {
                    observed_at: row.get(0)?,
                    session_id: row.get(1)?,
                    model: row.get(2)?,
                    counts: TokenCounts {
                        input_tokens: row.get::<_, i64>(3)?.max(0) as u64,
                        cached_input_tokens: row.get::<_, i64>(4)?.max(0) as u64,
                        cache_write_input_tokens: row.get::<_, i64>(5)?.max(0) as u64,
                        output_tokens: row.get::<_, i64>(6)?.max(0) as u64,
                        reasoning_output_tokens: row.get::<_, i64>(7)?.max(0) as u64,
                        total_tokens: row.get::<_, i64>(8)?.max(0) as u64,
                    },
                    event_ordinal: 0,
                })
            })
            .map_err(|error| error.to_string())?;

        let mut totals = TokenCounts::default();
        let mut models = BTreeMap::<String, TokenCounts>::new();
        let attributions = load_attributions(connection)?;
        let accounts = load_accounts(connection)?;
        let mut sessions = BTreeMap::<(String, String), SessionUsage>::new();
        let mut updated_at = None;
        for row in rows {
            let event = row.map_err(|error| error.to_string())?;
            totals.add_assign(&event.counts);
            models
                .entry(event.model.clone())
                .or_default()
                .add_assign(&event.counts);
            let session = sessions
                .entry((event.session_id.clone(), event.model.clone()))
                .or_insert_with(|| SessionUsage {
                    session_id: event.session_id.clone(),
                    model: event.model.clone(),
                    first_observed_at: event.observed_at.clone(),
                    last_observed_at: event.observed_at.clone(),
                    counts: TokenCounts::default(),
                    assignment: attributions.get(&event.session_id).cloned().unwrap_or_else(|| {
                        SessionAttribution {
                            account: None,
                            source: AttributionSource::Unassigned,
                            assigned_at: event.observed_at.clone(),
                            evidence_source: None,
                            evidence_observed_at: None,
                        }
                    }),
                });
            session.last_observed_at = event.observed_at.clone();
            session.counts.add_assign(&event.counts);
            updated_at = Some(event.observed_at);
        }
        Ok(TokenUsageData {
            totals,
            models: models
                .into_iter()
                .map(|(model, counts)| ModelUsage { model, counts })
                .collect(),
            sessions: sessions.into_values().collect(),
            accounts,
            updated_at: updated_at.unwrap_or_else(|| Utc::now().to_rfc3339()),
        })
    })
}

fn load_accounts(connection: &rusqlite::Connection) -> Result<Vec<TokenAccount>, String> {
    let mut statement = connection
        .prepare("SELECT account_key, display_name FROM token_accounts ORDER BY display_name, account_key")
        .map_err(|error| error.to_string())?;
    statement
        .query_map([], |row| {
            Ok(TokenAccount {
                account_key: row.get(0)?,
                display_name: row.get(1)?,
            })
        })
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())
}

fn load_attributions(
    connection: &rusqlite::Connection,
) -> Result<BTreeMap<String, SessionAttribution>, String> {
    let mut statement = connection
        .prepare(
            "SELECT attribution.session_id, attribution.attribution_source,
                    attribution.assigned_at_utc, attribution.evidence_source,
                    attribution.evidence_observed_at_utc,
                    account.account_key, account.display_name
             FROM token_session_attributions attribution
             LEFT JOIN token_accounts account ON account.account_key = attribution.account_key",
        )
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map([], |row| {
            let source = match row.get::<_, String>(1)?.as_str() {
                "activeAccount" => AttributionSource::ActiveAccount,
                "manual" => AttributionSource::Manual,
                _ => AttributionSource::Unassigned,
            };
            let account_key = row.get::<_, Option<String>>(5)?;
            let display_name = row.get::<_, Option<String>>(6)?;
            Ok((
                row.get::<_, String>(0)?,
                SessionAttribution {
                    account: account_key
                        .zip(display_name)
                        .map(|(account_key, display_name)| TokenAccount {
                            account_key,
                            display_name,
                        }),
                    source,
                    assigned_at: row.get(2)?,
                    evidence_source: row.get(3)?,
                    evidence_observed_at: row.get(4)?,
                },
            ))
        })
        .map_err(|error| error.to_string())?;
    rows.collect::<Result<BTreeMap<_, _>, _>>()
        .map_err(|error| error.to_string())
}
