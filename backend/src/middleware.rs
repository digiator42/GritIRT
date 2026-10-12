use std::sync::Arc;

use async_trait::async_trait;
use gritshield::http::request::HttpMethod;
use gritshield::http::response::Response;
use gritshield::middleware::{Middleware, MiddlewareResult};
use gritshield::prelude::RequestContext;
use serde_json::json;

use crate::state::AppState;

/// Require `X-API-Key` (or `?api_key=`) when `IRT_API_KEY` is configured.
/// `EventSource` cannot set headers, so the query-param form is supported for
/// the SSE stream.
pub struct ApiKeyMiddleware {
    expected: Option<String>,
}

impl ApiKeyMiddleware {
    pub fn new(state: Arc<AppState>) -> Self {
        Self {
            expected: state.api_key.clone(),
        }
    }
}

#[async_trait]
impl Middleware for ApiKeyMiddleware {
    async fn on_request(&self, ctx: &mut RequestContext) -> MiddlewareResult {
        if let Some(expected) = &self.expected {
            let provided = ctx
                .header("x-api-key")
                .map(ToOwned::to_owned)
                .or_else(|| ctx.query_param("api_key").map(ToOwned::to_owned))
                .unwrap_or_default();

            if provided.is_empty() || provided != *expected {
                return MiddlewareResult::Error(Response::json_unauthorized(&json!({
                    "error": "unauthorized",
                    "detail": "missing or invalid X-API-Key header (or ?api_key= query param)"
                })));
            }
        }
        MiddlewareResult::Next(None)
    }
}

/// Cap the size of write payloads. `ctx.raw_body` is fully buffered before the
/// middleware chain runs, so this is a cheap length check.
pub struct BodyLimitMiddleware {
    max_bytes: usize,
}

impl BodyLimitMiddleware {
    pub fn new(max_bytes: usize) -> Self {
        Self { max_bytes }
    }
}

#[async_trait]
impl Middleware for BodyLimitMiddleware {
    async fn on_request(&self, ctx: &mut RequestContext) -> MiddlewareResult {
        if ctx.req.method != HttpMethod::GET && ctx.raw_body.len() > self.max_bytes {
            let mut res = Response::json_bad_request(&json!({
                "error": "payload_too_large",
                "max": self.max_bytes
            }));
            res.status = 413;
            MiddlewareResult::Error(res)
        } else {
            MiddlewareResult::Next(None)
        }
    }
}

/// Attach hardening headers to the final response and expose request duration.
pub struct HttpSecurityMiddleware;

impl HttpSecurityMiddleware {
    pub fn new() -> Self {
        Self
    }
}

impl Default for HttpSecurityMiddleware {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Middleware for HttpSecurityMiddleware {
    async fn on_request(&self, _ctx: &mut RequestContext) -> MiddlewareResult {
        MiddlewareResult::Next(None)
    }

    async fn on_response(&self, ctx: &RequestContext, res: &mut Response) {
        push_header(res, "X-Content-Type-Options", "nosniff");
        push_header(res, "X-Frame-Options", "DENY");
        push_header(res, "Referrer-Policy", "no-referrer");
        push_header(res, "Cache-Control", "no-store");
        push_header(res, "Server", "gritirt-backend");
        res.headers.push((
            "X-IRT-Response-Time-Ms".to_string(),
            ctx.start_time.elapsed().as_millis().to_string(),
        ));
    }
}

fn push_header(res: &mut Response, key: &str, value: impl Into<String>) {
    if !res.headers.iter().any(|(k, _)| k.eq_ignore_ascii_case(key)) {
        res.headers.push((key.to_string(), value.into()));
    }
}