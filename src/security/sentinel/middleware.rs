/// Tower middleware for inbound unicode sanitization.
///
/// Intercepts HTTP request bodies (webhook payloads) and sanitizes them
/// through the Sentinel unicode pipeline before they reach handlers.
/// Placed after RequestBodyLimitLayer (size enforced first) and before handlers.
///
/// See `docs/sentinel-gateway-redaction-design.md` — "Inbound: Unicode Sanitization".
use std::sync::Arc;

use axum::body::Bytes;
use axum::{
    body::Body,
    http::{Request, Response},
};
use http_body_util::BodyExt;
use tower::{Layer, Service};

use super::sanitize_config::SanitizationConfig;
use super::sanitizer::SanitizationEngine;

/// Tower Layer that wraps services with inbound unicode sanitization.
#[derive(Clone)]
pub struct SentinelInboundLayer {
    engine: Arc<SanitizationEngine>,
}

impl SentinelInboundLayer {
    /// Create a new inbound sanitization layer.
    pub fn new(config: SanitizationConfig) -> Self {
        Self {
            engine: Arc::new(SanitizationEngine::new(config)),
        }
    }

    /// Create with default config.
    pub fn default_config() -> Self {
        Self::new(SanitizationConfig::default())
    }
}

impl<S> Layer<S> for SentinelInboundLayer {
    type Service = SentinelInboundService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        SentinelInboundService {
            inner,
            engine: self.engine.clone(),
        }
    }
}

/// The Tower Service produced by `SentinelInboundLayer`.
#[derive(Clone)]
pub struct SentinelInboundService<S> {
    inner: S,
    engine: Arc<SanitizationEngine>,
}

impl<S> Service<Request<Body>> for SentinelInboundService<S>
where
    S: Service<Request<Body>, Response = Response<Body>> + Clone + Send + 'static,
    S::Future: Send + 'static,
    S::Error: Send + 'static,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Self::Response, Self::Error>> + Send>,
    >;

    fn poll_ready(
        &mut self,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Request<Body>) -> Self::Future {
        let engine = self.engine.clone();
        let mut inner = self.inner.clone();

        Box::pin(async move {
            // Only sanitize POST/PUT/PATCH bodies (webhooks, API writes)
            let method = req.method().clone();
            if !matches!(
                method,
                axum::http::Method::POST | axum::http::Method::PUT | axum::http::Method::PATCH
            ) {
                return inner.call(req).await;
            }

            // Check content type — only sanitize text/json bodies
            let content_type = req
                .headers()
                .get(axum::http::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("");

            let is_text = content_type.contains("json")
                || content_type.contains("text")
                || content_type.contains("x-www-form-urlencoded");

            if !is_text {
                return inner.call(req).await;
            }

            // Extract body, sanitize, rebuild request
            let (parts, body) = req.into_parts();

            let body_bytes = match body.collect().await {
                Ok(collected) => collected.to_bytes(),
                Err(_) => {
                    // If we can't read the body, pass through empty
                    tracing::warn!("sentinel: failed to read request body for sanitization");
                    Bytes::new()
                }
            };

            // Try to sanitize as UTF-8 text
            let sanitized_bytes = match std::str::from_utf8(&body_bytes) {
                Ok(text) => {
                    let sanitized = engine.sanitize(text);
                    Bytes::from(sanitized.into_owned())
                }
                Err(_) => {
                    // Not valid UTF-8, pass through unchanged
                    body_bytes
                }
            };

            let new_body = Body::from(sanitized_bytes);
            let new_req = Request::from_parts(parts, new_body);

            inner.call(new_req).await
        })
    }
}

/// Sanitize a WebSocket text frame through the Sentinel pipeline.
///
/// Use this in the WS handler to sanitize inbound frames before processing.
pub fn sanitize_ws_frame(engine: &SanitizationEngine, text: &str) -> String {
    engine.sanitize(text).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::Body,
        http::{Request, StatusCode},
        routing::post,
        Router,
    };
    use tower::ServiceExt;

    async fn echo_handler(body: String) -> String {
        body
    }

    fn test_app() -> Router {
        Router::new()
            .route("/webhook", post(echo_handler))
            .layer(SentinelInboundLayer::default_config())
    }

    #[tokio::test]
    async fn sanitizes_webhook_body() {
        let app = test_app();

        // Body with zero-width space injection
        let body = "hello\u{200B}world";
        let req = Request::builder()
            .method("POST")
            .uri("/webhook")
            .header("content-type", "application/json")
            .body(Body::from(body))
            .unwrap();

        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body_bytes = response.into_body().collect().await.unwrap().to_bytes();
        let body_str = std::str::from_utf8(&body_bytes).unwrap();
        assert_eq!(body_str, "helloworld");
    }

    #[tokio::test]
    async fn get_requests_bypass_sanitization() {
        let app = Router::new()
            .route("/health", axum::routing::get(|| async { "ok" }))
            .layer(SentinelInboundLayer::default_config());

        let req = Request::builder()
            .method("GET")
            .uri("/health")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn non_text_content_type_bypasses() {
        let app = test_app();

        let req = Request::builder()
            .method("POST")
            .uri("/webhook")
            .header("content-type", "application/octet-stream")
            .body(Body::from("binary\u{200B}data"))
            .unwrap();

        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn sanitizes_json_body_with_bidi() {
        let app = test_app();

        let body = format!(
            r#"{{"message": "hello{}world"}}"#,
            '\u{202E}' // RTL override
        );
        let req = Request::builder()
            .method("POST")
            .uri("/webhook")
            .header("content-type", "application/json")
            .body(Body::from(body))
            .unwrap();

        let response = app.oneshot(req).await.unwrap();
        let body_bytes = response.into_body().collect().await.unwrap().to_bytes();
        let body_str = std::str::from_utf8(&body_bytes).unwrap();
        assert!(
            !body_str.contains('\u{202E}'),
            "bidi not sanitized: {body_str}"
        );
        assert!(body_str.contains("hello world")); // replaced with space
    }

    #[tokio::test]
    async fn clean_ascii_body_passes_through() {
        let app = test_app();

        let body = r#"{"message": "Hello, this is clean ASCII"}"#;
        let req = Request::builder()
            .method("POST")
            .uri("/webhook")
            .header("content-type", "application/json")
            .body(Body::from(body))
            .unwrap();

        let response = app.oneshot(req).await.unwrap();
        let body_bytes = response.into_body().collect().await.unwrap().to_bytes();
        let body_str = std::str::from_utf8(&body_bytes).unwrap();
        assert_eq!(body_str, body);
    }

    // --- WS frame sanitization ---

    #[test]
    fn ws_frame_sanitization() {
        let engine = SanitizationEngine::new(SanitizationConfig::default());
        let result = sanitize_ws_frame(&engine, "hello\u{200B}\u{202E}world");
        assert_eq!(result, "hello world");
    }

    #[test]
    fn ws_clean_frame_unchanged() {
        let engine = SanitizationEngine::new(SanitizationConfig::default());
        let result = sanitize_ws_frame(&engine, "clean message");
        assert_eq!(result, "clean message");
    }
}
