use axum::{
    body::Body,
    http::{header::CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use serde_json::json;

const SESSION_HEADER: HeaderName = HeaderName::from_static("x-creavor-session-id");

pub fn strip_session_header(headers: &mut HeaderMap) {
    headers.remove(SESSION_HEADER);
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
                    "code": serde_json::Value::Null,
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
                    "code": null,
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
                    "code": null,
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
}
