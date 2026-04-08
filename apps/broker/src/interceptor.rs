use axum::{
    body::Body,
    http::{header::CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use serde_json::json;

const SESSION_HEADER: HeaderName = HeaderName::from_static("x-creavor-session-id");
const RUNTIME_HEADER: HeaderName = HeaderName::from_static("x-creavor-runtime");

pub fn strip_session_header(headers: &mut HeaderMap) {
    headers.remove(SESSION_HEADER);
}

pub fn strip_runtime_header(headers: &mut HeaderMap) {
    headers.remove(RUNTIME_HEADER);
}

pub fn strip_creavor_headers(headers: &mut HeaderMap) {
    strip_session_header(headers);
    strip_runtime_header(headers);
}

pub fn anthropic_block_response(message: &str) -> Response {
    anthropic_block_response_with_status(StatusCode::BAD_REQUEST, message)
}

pub fn anthropic_block_response_with_status(status: StatusCode, message: &str) -> Response {
    (
        status,
        [(CONTENT_TYPE, HeaderValue::from_static("application/json"))],
        Body::from(
            json!({
                "type": "error",
                "error": {
                    "type": "invalid_request_error",
                    "message": message,
                }
            })
            .to_string(),
        ),
    )
        .into_response()
}

pub fn openai_block_response(message: &str) -> Response {
    openai_block_response_with_status(StatusCode::BAD_REQUEST, message)
}

pub fn openai_block_response_with_status(status: StatusCode, message: &str) -> Response {
    (
        status,
        [(CONTENT_TYPE, HeaderValue::from_static("application/json"))],
        Body::from(
            json!({
                "error": {
                    "message": message,
                    "type": "invalid_request_error",
                    "param": serde_json::Value::Null,
                    "code": "content_policy_violation",
                }
            })
            .to_string(),
        ),
    )
        .into_response()
}

pub fn gemini_block_response(message: &str) -> Response {
    gemini_block_response_with_status(StatusCode::BAD_REQUEST, message)
}

pub fn gemini_block_response_with_status(status: StatusCode, message: &str) -> Response {
    // Same as openai_block_response_with_status — Gemini uses OpenAI-compatible format
    (
        status,
        [(CONTENT_TYPE, HeaderValue::from_static("application/json"))],
        Body::from(
            json!({
                "error": {
                    "message": message,
                    "type": "invalid_request_error",
                    "param": serde_json::Value::Null,
                    "code": "content_policy_violation",
                }
            })
            .to_string(),
        ),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use http_body_util::BodyExt;

    async fn body_as_json(response: Response) -> serde_json::Value {
        let body = response.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice(&body).unwrap()
    }

    #[tokio::test]
    async fn anthropic_block_response_uses_provider_envelope() {
        let response = anthropic_block_response("blocked");

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            response.headers().get(CONTENT_TYPE).unwrap(),
            "application/json"
        );
        assert_eq!(
            body_as_json(response).await,
            json!({
                "type": "error",
                "error": {
                    "type": "invalid_request_error",
                    "message": "blocked",
                }
            })
        );
    }

    #[tokio::test]
    async fn openai_block_response_uses_provider_envelope() {
        let response = openai_block_response("blocked");

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            response.headers().get(CONTENT_TYPE).unwrap(),
            "application/json"
        );
        assert_eq!(
            body_as_json(response).await,
            json!({
                "error": {
                    "message": "blocked",
                    "type": "invalid_request_error",
                    "param": null,
                    "code": "content_policy_violation",
                }
            })
        );
    }

    #[tokio::test]
    async fn openai_block_response_with_status_overrides_status_code() {
        let response = openai_block_response_with_status(StatusCode::FORBIDDEN, "blocked");

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert_eq!(
            response.headers().get(CONTENT_TYPE).unwrap(),
            "application/json"
        );
        assert_eq!(
            body_as_json(response).await,
            json!({
                "error": {
                    "message": "blocked",
                    "type": "invalid_request_error",
                    "param": null,
                    "code": "content_policy_violation",
                }
            })
        );
    }

    #[test]
    fn strip_session_header_removes_creavor_session_id() {
        let mut headers = HeaderMap::new();
        headers.insert(SESSION_HEADER, HeaderValue::from_static("session-123"));

        strip_session_header(&mut headers);

        assert!(headers.get(SESSION_HEADER).is_none());
    }

    #[test]
    fn strip_creavor_headers_removes_both_session_and_runtime() {
        let mut headers = HeaderMap::new();
        headers.insert(SESSION_HEADER, HeaderValue::from_static("session-123"));
        headers.insert(RUNTIME_HEADER, HeaderValue::from_static("claude-code"));
        headers.insert(
            HeaderName::from_static("authorization"),
            HeaderValue::from_static("Bearer token"),
        );

        strip_creavor_headers(&mut headers);

        assert!(headers.get(SESSION_HEADER).is_none());
        assert!(headers.get(RUNTIME_HEADER).is_none());
        assert!(headers.get("authorization").is_some());
    }

    #[tokio::test]
    async fn gemini_block_response_uses_content_policy_violation_code() {
        let response = gemini_block_response("blocked");

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            response.headers().get(CONTENT_TYPE).unwrap(),
            "application/json"
        );
        assert_eq!(
            body_as_json(response).await,
            json!({
                "error": {
                    "message": "blocked",
                    "type": "invalid_request_error",
                    "param": null,
                    "code": "content_policy_violation",
                }
            })
        );
    }
}
