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
    collections::VecDeque,
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
            expected_token,
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

    fn allow_event(&self) -> bool {
        self.rate_limiter.lock().unwrap().allow(Instant::now())
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

    if !state.allow_event() {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(json!({ "error": "rate_limited" })),
        )
            .into_response();
    }

    let correlation_id = correlation_id_for_event(&headers, &payload);
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

struct LocalRateLimiter {
    window: Duration,
    limit: usize,
    recent_events: VecDeque<Instant>,
}

impl LocalRateLimiter {
    fn new(window: Duration, limit: usize) -> Self {
        Self {
            window,
            limit,
            recent_events: VecDeque::with_capacity(limit),
        }
    }

    fn allow(&mut self, now: Instant) -> bool {
        while let Some(oldest) = self.recent_events.front().copied() {
            if now.duration_since(oldest) < self.window {
                break;
            }
            self.recent_events.pop_front();
        }

        if self.recent_events.len() >= self.limit {
            return false;
        }

        self.recent_events.push_back(now);
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

        assert!(limiter.allow(now));
        assert!(limiter.allow(now));
        assert!(!limiter.allow(now));
    }

    #[test]
    fn local_rate_limiter_recovers_after_window_expires() {
        let mut limiter = LocalRateLimiter::new(Duration::from_secs(1), 1);
        let now = Instant::now();

        assert!(limiter.allow(now));
        assert!(!limiter.allow(now));
        assert!(limiter.allow(now + Duration::from_secs(1)));
    }
}
