use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use sea_orm::DatabaseConnection;
use tokio::sync::Mutex;

use crate::feed::Feed;

/// Outbound connector description: where an MCP tool on a named system lives.
#[derive(Clone, Debug)]
pub struct Connector {
    /// Base URL, e.g. `http://127.0.0.1:8020`.
    pub base_url: String,
    /// Optional API key forwarded as `X-API-Key` on MCP calls.
    pub api_key: Option<String>,
}

/// Application-wide shared state, registered once into the GritShield DI
/// container (`AutoWire::component(AppState)`) and resolved on demand by
/// handlers, middlewares and MCP tools via [`resolve`].
#[derive(Clone)]
pub struct AppState {
    pub db: Arc<DatabaseConnection>,
    pub feed: Arc<Feed>,
    /// When `Some`, requests must present `X-API-Key` (or `?api_key=`).
    pub api_key: Option<String>,
    /// Named outbound connectors used by playbook MCP steps.
    pub connectors: Arc<HashMap<String, Connector>>,
    /// Default correlation/dedup window in milliseconds.
    pub dedup_window_ms: i64,
    /// Reusable outbound HTTP client for connector/HTTP actions.
    pub client: reqwest::Client,
    /// Run ids currently executing, so `resume` never double-drives a run.
    pub active_runs: Arc<Mutex<HashSet<i64>>>,
}

/// Default correlation/dedup window (5 minutes) in milliseconds.
pub const DEFAULT_DEDUP_WINDOW_MS: i64 = 5 * 60 * 1000;

/// Resolve the registered application state from the global container.
pub fn resolve() -> Arc<AppState> {
    use gritshield::core::ioc::CONTEXT;
    CONTEXT
        .resolve::<AppState>()
        .expect("AppState was not registered; this must be the first boot step")
}