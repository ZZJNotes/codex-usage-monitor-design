use std::collections::BTreeMap;

use chrono::Utc;
use rusqlite::{params, params_from_iter, types::Value as SqlValue};

use crate::database::Database;

use super::{
    ModelUsage, SessionUsage, TokenCounts, TokenEvent, TokenUsageData, TokenUsageFilters,
    hex_digest,
};

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
            updated_at: updated_at.unwrap_or_else(|| Utc::now().to_rfc3339()),
        })
    })
}
