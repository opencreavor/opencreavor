use crate::{
    audit::{correlation_id_for_event, event_type_from_payload, sanitize_local_event_payload},
    storage::AuditStorage,
};
use axum::{
    extract::{Json, State},
    http::{header::AUTHORIZATION, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

const RATE_LIMIT_WINDOW: Duration = Duration::from_secs(1);
const RATE_LIMIT_MAX_EVENTS: usize = 32;

#[derive(Clone)]
pub struct EventsState {
    expected_token: Option<String>,
    storage: Arc<Mutex<AuditStorage>>,
    rate_limiter: Arc<Mutex<LocalRateLimiter>>,
}

impl EventsState {
    pub fn new(expected_token: Option<String>, storage: AuditStorage) -> Self {
        Self {
            expected_token: normalize_expected_token(expected_token),
            storage: Arc::new(Mutex::new(storage)),
            rate_limiter: Arc::new(Mutex::new(LocalRateLimiter::new(
                RATE_LIMIT_WINDOW,
                RATE_LIMIT_MAX_EVENTS,
            ))),
        }
    }

    fn is_authorized(&self, headers: &HeaderMap) -> bool {
        let Some(expected_token) = self.expected_token.as_deref() else {
            return false;
        };

        let Some(header) = headers.get(AUTHORIZATION) else {
            return false;
        };

        let Ok(header) = header.to_str() else {
            return false;
        };

        let Some(token) = header.strip_prefix("Bearer ") else {
            return false;
        };

        token == expected_token
    }

    fn allow_event(&self, key: &str) -> bool {
        self.rate_limiter.lock().unwrap().allow(key, Instant::now())
    }

    fn persist_event(&self, payload: &Value, correlation_id: &str) -> anyhow::Result<()> {
        let serialized = serde_json::to_string(payload)?;
        self.storage.lock().unwrap().insert_event(
            event_type_from_payload(payload),
            Some(correlation_id),
            Some(&serialized),
        )?;
        Ok(())
    }
}

pub async fn post_events(
    State(state): State<EventsState>,
    headers: HeaderMap,
    Json(payload): Json<Value>,
) -> Response {
    if !state.is_authorized(&headers) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "unauthorized" })),
        )
            .into_response();
    }

    let correlation_id = correlation_id_for_event(&headers, &payload);

    if !state.allow_event(&correlation_id) {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(json!({ "error": "rate_limited" })),
        )
            .into_response();
    }
    let sanitized_payload = sanitize_local_event_payload(payload, correlation_id.clone());

    match state.persist_event(&sanitized_payload, &correlation_id) {
        Ok(()) => (StatusCode::ACCEPTED, Json(json!({ "accepted": true }))).into_response(),
        Err(error) => {
            tracing::error!(?error, "failed to persist local event");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "persistence_failed" })),
            )
                .into_response()
        }
    }
}

fn normalize_expected_token(expected_token: Option<String>) -> Option<String> {
    expected_token.and_then(|token| {
        let trimmed = token.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
}

struct LocalRateLimiter {
    window: Duration,
    limit: usize,
    recent_events: HashMap<String, VecDeque<Instant>>,
}

impl LocalRateLimiter {
    fn new(window: Duration, limit: usize) -> Self {
        Self {
            window,
            limit,
            recent_events: HashMap::new(),
        }
    }

    fn allow(&mut self, key: &str, now: Instant) -> bool {
        let events = self
            .recent_events
            .entry(key.to_string())
            .or_insert_with(|| VecDeque::with_capacity(self.limit));

        while let Some(oldest) = events.front().copied() {
            if now.duration_since(oldest) < self.window {
                break;
            }
            events.pop_front();
        }

        if events.len() >= self.limit {
            return false;
        }

        events.push_back(now);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_rate_limiter_blocks_after_limit_is_reached() {
        let mut limiter = LocalRateLimiter::new(Duration::from_secs(1), 2);
        let now = Instant::now();

        assert!(limiter.allow("session-a", now));
        assert!(limiter.allow("session-a", now));
        assert!(!limiter.allow("session-a", now));
    }

    #[test]
    fn local_rate_limiter_recovers_after_window_expires() {
        let mut limiter = LocalRateLimiter::new(Duration::from_secs(1), 1);
        let now = Instant::now();

        assert!(limiter.allow("session-a", now));
        assert!(!limiter.allow("session-a", now));
        assert!(limiter.allow("session-a", now + Duration::from_secs(1)));
    }

    #[test]
    fn local_rate_limiter_is_scoped_per_key() {
        let mut limiter = LocalRateLimiter::new(Duration::from_secs(1), 1);
        let now = Instant::now();

        assert!(limiter.allow("session-a", now));
        assert!(!limiter.allow("session-a", now));
        assert!(limiter.allow("session-b", now));
    }

    #[test]
    fn normalize_expected_token_rejects_blank_values() {
        assert_eq!(normalize_expected_token(Some(String::new())), None);
        assert_eq!(normalize_expected_token(Some("   ".to_string())), None);
        assert_eq!(
            normalize_expected_token(Some(" secret ".to_string())),
            Some("secret".to_string())
        );
    }
}
