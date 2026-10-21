//! What the forges answer, read alike: a GraphQL reply's data or its errors, a REST answer's
//! JSON, and a refusal turned into its [`ForgeError`]: a rate limit used up (by either forge's
//! headers, or GraphQL's `RATE_LIMITED`) says until when, a repository that isn't there (or
//! isn't visible) says so, and anything else the host refuses carries its own reason.

use std::time::{SystemTime, UNIX_EPOCH};

use egit_git::CancelToken;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::json;

use crate::account::Account;
use crate::error::ForgeError;
use crate::http::Answer;

/// Runs a GraphQL query, reading its data or its errors.
pub(crate) fn graphql<T: DeserializeOwned>(
    account: &Account,
    query: &str,
    variables: &serde_json::Value,
    what: &str,
    cancel: &CancelToken,
) -> Result<T, ForgeError> {
    let body = json!({ "query": query, "variables": variables }).to_string();
    let url = account.host().graphql_url();
    let answer = account.send(&url, Some(body.as_bytes()), cancel)?;
    let (remaining, reset) = (answer.remaining, answer.reset);
    let reply: GraphQlReply<T> = read_answer(account, &answer, what)?;
    if let Some(error) = reply.errors.first() {
        let host = account.host().name().to_owned();
        return Err(match error.kind.as_deref() {
            Some("RATE_LIMITED") => ForgeError::RateLimited {
                host,
                reset: reset.filter(|_| remaining.is_none_or(|left| left == 0)),
            },
            Some("NOT_FOUND") => ForgeError::NotFound {
                host,
                what: what.to_owned(),
            },
            Some("FORBIDDEN" | "INSUFFICIENT_SCOPES") => ForgeError::Forbidden {
                host,
                message: error.message.clone(),
            },
            _ => ForgeError::Said {
                host,
                message: error.message.clone(),
            },
        });
    }
    reply.data.ok_or_else(|| ForgeError::Malformed {
        host: account.host().name().to_owned(),
        detail: "an answer without data".to_owned(),
    })
}

/// Reads a successful answer's JSON, or turns a refusal into its error.
pub(crate) fn read_answer<T: DeserializeOwned>(
    account: &Account,
    answer: &Answer,
    what: &str,
) -> Result<T, ForgeError> {
    let host = account.host().name().to_owned();
    match answer.status {
        200..=299 => serde_json::from_slice(&answer.body).map_err(|error| ForgeError::Malformed {
            host,
            detail: error.to_string(),
        }),
        403 | 429 if answer.remaining == Some(0) || answer.retry_after.is_some() => {
            let reset = answer.reset.or_else(|| {
                answer
                    .retry_after
                    .map(|after| now().saturating_add(after.max(0)))
            });
            Err(ForgeError::RateLimited { host, reset })
        }
        403 => Err(ForgeError::Forbidden {
            host,
            message: reason(&answer.body),
        }),
        404 => Err(ForgeError::NotFound {
            host,
            what: what.to_owned(),
        }),
        status => Err(ForgeError::Status {
            host,
            status,
            message: reason(&answer.body),
        }),
    }
}

/// A refusal's reason, as either forge words it: GitHub's `message` with a validation
/// failure's own messages; GitLab's `message` (a sentence, a list, or each field's), or its
/// `error` and `error_description`; else the answer's start.
pub(crate) fn reason(body: &[u8]) -> String {
    let said: serde_json::Value = serde_json::from_slice(body).unwrap_or_default();
    let text = |value: &serde_json::Value| -> Vec<String> {
        match value {
            serde_json::Value::String(text) => vec![text.clone()],
            serde_json::Value::Array(items) => items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_owned))
                .collect(),
            serde_json::Value::Object(fields) => fields
                .iter()
                .flat_map(|(field, value)| {
                    let messages: Vec<String> = match value {
                        serde_json::Value::String(text) => vec![text.clone()],
                        serde_json::Value::Array(items) => items
                            .iter()
                            .filter_map(|item| item.as_str().map(str::to_owned))
                            .collect(),
                        _ => Vec::new(),
                    };
                    messages
                        .into_iter()
                        .map(move |message| format!("{field} {message}"))
                })
                .collect(),
            _ => Vec::new(),
        }
    };
    let details: Vec<String> = said
        .get("errors")
        .and_then(serde_json::Value::as_array)
        .map(|errors| {
            errors
                .iter()
                .filter_map(|error| error.get("message").and_then(serde_json::Value::as_str))
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    if !details.is_empty() {
        return details.join(" ");
    }
    for key in ["message", "error_description", "error"] {
        let messages = said.get(key).map(text).unwrap_or_default();
        if !messages.is_empty() {
            return messages.join(" ");
        }
    }
    String::from_utf8_lossy(body).chars().take(200).collect()
}

pub(crate) fn not_found(account: &Account, what: String) -> ForgeError {
    ForgeError::NotFound {
        host: account.host().name().to_owned(),
        what,
    }
}

pub(crate) const fn cancelled() -> ForgeError {
    ForgeError::Cancelled
}

/// Seconds since the Unix epoch.
pub(crate) fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|since| i64::try_from(since.as_secs()).ok())
        .unwrap_or(0)
}

/// An ISO 8601 time as the forges write it, in UTC (`2024-05-01T10:20:30Z`, maybe with a
/// fraction of a second), in seconds since the Unix epoch.
pub(crate) fn parse_time(text: &str) -> Option<i64> {
    let number = |range: std::ops::Range<usize>| -> Option<i64> {
        let digits = text.get(range)?;
        digits
            .bytes()
            .all(|byte| byte.is_ascii_digit())
            .then(|| digits.parse().ok())
            .flatten()
    };
    let separators = [(4, b'-'), (7, b'-'), (10, b'T'), (13, b':'), (16, b':')];
    if !separators
        .iter()
        .all(|&(at, byte)| text.as_bytes().get(at) == Some(&byte))
    {
        return None;
    }
    let (year, month, day) = (number(0..4)?, number(5..7)?, number(8..10)?);
    let (hour, minute, second) = (number(11..13)?, number(14..16)?, number(17..19)?);
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || hour > 23 || minute > 59 {
        return None;
    }
    // Days from the civil date (Howard Hinnant's algorithm), in years starting in March.
    let year = if month <= 2 {
        year.checked_sub(1)?
    } else {
        year
    };
    let era = year.div_euclid(400);
    let of_era = year.rem_euclid(400);
    let month_index = if month > 2 {
        month.checked_sub(3)?
    } else {
        month.checked_add(9)?
    };
    let of_year = 153_i64
        .checked_mul(month_index)?
        .checked_add(2)?
        .checked_div(5)?
        .checked_add(day)?
        .checked_sub(1)?;
    let of_cycle = of_era
        .checked_mul(365)?
        .checked_add(of_era.checked_div(4)?)?
        .checked_sub(of_era.checked_div(100)?)?
        .checked_add(of_year)?;
    let days = era
        .checked_mul(146_097)?
        .checked_add(of_cycle)?
        .checked_sub(719_468)?;
    days.checked_mul(86_400)?
        .checked_add(hour.checked_mul(3_600)?)?
        .checked_add(minute.checked_mul(60)?)?
        .checked_add(second)
}

#[derive(Deserialize)]
struct GraphQlReply<T> {
    data: Option<T>,
    #[serde(default)]
    errors: Vec<GraphQlError>,
}

#[derive(Deserialize)]
struct GraphQlError {
    #[serde(rename = "type")]
    kind: Option<String>,
    #[serde(default)]
    message: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_are_read() {
        assert_eq!(parse_time("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_time("2024-02-29T12:34:56Z"), Some(1_709_210_096));
        assert_eq!(parse_time("2024-02-29T12:34:56.789Z"), Some(1_709_210_096));
        assert_eq!(parse_time("2000-03-01T00:00:00Z"), Some(951_868_800));
        assert_eq!(parse_time("2024-13-01T00:00:00Z"), None);
        assert_eq!(parse_time("yesterday"), None);
    }

    #[test]
    fn a_refusal_s_own_reasons_are_given() {
        let body = br#"{"message":"Validation Failed","errors":[{"resource":"PullRequest",
            "code":"custom","message":"A pull request already exists for ada:topic."}]}"#;
        assert_eq!(reason(body), "A pull request already exists for ada:topic.");
        assert_eq!(reason(br#"{"message":"Not Found"}"#), "Not Found");
        assert_eq!(
            reason(br#"{"message":["Another open merge request already exists for this source branch: !5"]}"#),
            "Another open merge request already exists for this source branch: !5"
        );
        assert_eq!(
            reason(br#"{"message":{"title":["can't be blank"]}}"#),
            "title can't be blank"
        );
        assert_eq!(
            reason(br#"{"error":"insufficient_scope","error_description":"The request requires higher privileges."}"#),
            "The request requires higher privileges."
        );
        assert_eq!(reason(b"<html>"), "<html>");
    }
}
